use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context as _;
use futures_util::StreamExt as _;
use tokio::signal::unix::{signal, SignalKind};
use zbus::proxy;
use zbus::Connection;

use facegate_core::camera::V4lCamera;
use facegate_core::config::Config;
use facegate_core::error::FaceRsError;
use facegate_core::storage::AuthScope;

use crate::commands::broker;
use crate::commands::broker::frame_probe;

// ── D-Bus proxies ─────────────────────────────────────────────────────────────

#[proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1",
    gen_blocking = false
)]
trait Login1Manager {
    fn get_session(&self, session_id: &str) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

#[proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1",
    gen_blocking = false
)]
trait Login1Session {
    /// Signal fired when the session is locked.
    #[zbus(signal)]
    fn lock(&self) -> zbus::Result<()>;

    /// Signal fired when the session is unlocked (e.g. user typed their password).
    /// Renamed to avoid a name collision with the Unlock method below.
    #[zbus(signal, name = "Unlock")]
    fn session_unlocked(&self) -> zbus::Result<()>;

    /// Programmatically unlock the session (equivalent to `loginctl unlock-session`).
    fn unlock(&self) -> zbus::Result<()>;
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(config: Config) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
    rt.block_on(run_async(config))
}

async fn run_async(config: Config) -> anyhow::Result<()> {
    let username = std::env::var("USER")
        .context("USER env var not set — facegate-watch must run inside a user session")?;
    let session_id = std::env::var("XDG_SESSION_ID")
        .context("XDG_SESSION_ID not set — facegate-watch must run inside a user session")?;

    let conn = Connection::system()
        .await
        .context("cannot connect to system D-Bus")?;

    let manager = Login1ManagerProxy::new(&conn)
        .await
        .context("cannot create login1 manager proxy")?;

    let session_path = manager
        .get_session(&session_id)
        .await
        .with_context(|| format!("cannot get session path for XDG_SESSION_ID={session_id}"))?;

    let session = Login1SessionProxy::builder(&conn)
        .path(session_path.clone())
        .context("invalid session object path")?
        .build()
        .await
        .context("cannot create session proxy")?;

    let mut lock_stream = session
        .receive_lock()
        .await
        .context("cannot subscribe to Lock signal")?;

    let mut unlock_stream = session
        .receive_session_unlocked()
        .await
        .context("cannot subscribe to Unlock signal")?;

    tracing::info!(
        username,
        session_id,
        path = %session_path,
        "facegate-watch started"
    );

    // Track an in-progress scan so it can be cancelled on external unlock.
    let mut scan_cancel: Option<Arc<AtomicBool>> = None;

    // SIGTERM is what systemd sends on `systemctl --user stop`; ctrl_c covers
    // SIGINT for foreground use.
    let mut sigterm =
        signal(SignalKind::terminate()).context("failed to register SIGTERM handler")?;

    loop {
        tokio::select! {
            biased;

            // Graceful shutdown on SIGTERM or Ctrl-C.
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT — shutting down");
                if let Some(cancel) = scan_cancel.take() {
                    cancel.store(true, Ordering::Relaxed);
                }
                break;
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM — shutting down");
                if let Some(cancel) = scan_cancel.take() {
                    cancel.store(true, Ordering::Relaxed);
                }
                break;
            }

            lock = lock_stream.next() => {
                let Some(_) = lock else {
                    // Stream ended (D-Bus disconnect). Returning Err lets
                    // systemd's Restart=on-failure bring us back up.
                    if let Some(cancel) = scan_cancel.take() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    anyhow::bail!("system D-Bus Lock stream ended unexpectedly");
                };
                tracing::info!("session locked — starting face recognition");

                // Cancel any stale scan (shouldn't happen, but be defensive).
                if let Some(cancel) = scan_cancel.take() {
                    cancel.store(true, Ordering::Relaxed);
                }

                let cancel = Arc::new(AtomicBool::new(false));
                scan_cancel = Some(cancel.clone());

                let config = config.clone();
                let username = username.clone();
                let session_clone = session.clone();

                tokio::task::spawn_blocking(move || {
                    run_recognition(&config, &username, &session_clone, &cancel);
                });
            }

            unlock = unlock_stream.next() => {
                let Some(_) = unlock else {
                    if let Some(cancel) = scan_cancel.take() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    anyhow::bail!("system D-Bus Unlock stream ended unexpectedly");
                };
                tracing::info!("session unlocked externally — cancelling scan");
                if let Some(cancel) = scan_cancel.take() {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    Ok(())
}

// ── Recognition loop (blocking, runs on the thread-pool) ─────────────────────

fn run_recognition(
    config: &Config,
    username: &str,
    session: &Login1SessionProxy,
    cancel: &AtomicBool,
) {
    match broker::list_templates(username) {
        Ok(templates)
            if templates
                .iter()
                .any(|template| broker::summary_allows(template, AuthScope::Session)) => {}
        Ok(_) => {
            tracing::warn!(username, "no session templates — skipping face scan");
            return;
        }
        Err(e) => {
            tracing::error!(username, "broker template list error: {e}");
            return;
        }
    };

    if cancel.load(Ordering::Relaxed) {
        return;
    }

    let cross_check = config.camera.cross_check.enabled && config.camera.ir_device.is_some();
    let mut camera = match V4lCamera::open(
        &config.camera.device,
        config.camera.width,
        config.camera.height,
        config.camera.fps,
        config.camera.timeout_ms,
    ) {
        Ok(mut cam) => {
            cam.warmup(config.camera.warmup_frames);
            cam
        }
        Err(e) => {
            tracing::error!("cannot open camera: {e}");
            return;
        }
    };
    let mut ir_camera = if cross_check {
        let Some(ir_device) = config.camera.ir_device.as_deref() else {
            tracing::error!("cross-check is enabled but camera.ir_device is missing");
            return;
        };
        match V4lCamera::open(
            ir_device,
            config.camera.width,
            config.camera.height,
            config.camera.fps,
            config.camera.timeout_ms,
        ) {
            Ok(mut cam) => {
                cam.warmup(config.camera.warmup_frames);
                Some(cam)
            }
            Err(e) => {
                tracing::error!("cannot open IR camera: {e}");
                return;
            }
        }
    } else {
        None
    };

    let required = config.recognition.required_matches.max(1);
    let mut matches: u32 = 0;

    for attempt in 1..=config.recognition.max_attempts {
        if cancel.load(Ordering::Relaxed) {
            tracing::debug!("recognition cancelled (attempt {attempt})");
            return;
        }

        match camera.capture_frame() {
            Ok(frame) => {
                let result = if let Some(ir_camera) = ir_camera.as_mut() {
                    let rgb_probe = frame_probe(frame);
                    let ir_frame = match ir_camera.capture_frame() {
                        Ok(frame) => frame,
                        Err(FaceRsError::Timeout) => {
                            tracing::debug!(attempt, "IR capture timeout");
                            continue;
                        }
                        Err(e) => {
                            tracing::error!("IR capture error: {e}");
                            return;
                        }
                    };
                    broker::match_frame_pair(
                        username,
                        AuthScope::Session,
                        rgb_probe,
                        frame_probe(ir_frame),
                    )
                } else {
                    broker::match_frame(username, AuthScope::Session, frame_probe(frame))
                };
                match result {
                    Ok(result) if result.matched => {
                        matches += 1;
                        tracing::debug!(
                            attempt,
                            matches,
                            required,
                            username,
                            score = result.score,
                            "match accepted"
                        );
                        if matches >= required {
                            tracing::info!(username, "face recognised — unlocking session");
                            // Use the async D-Bus proxy from a blocking context.
                            let result =
                                tokio::runtime::Handle::current().block_on(session.unlock());
                            if let Err(e) = result {
                                tracing::error!("unlock call failed: {e}");
                            }
                            return;
                        }
                    }
                    Ok(result) => {
                        tracing::debug!(
                            attempt,
                            username,
                            score = result.score,
                            "attempt did not match"
                        );
                    }
                    Err(e) => {
                        tracing::error!("broker match error: {e}");
                        return;
                    }
                }
            }
            Err(FaceRsError::Timeout) => {
                tracing::debug!(attempt, "capture timeout");
            }
            Err(e) => {
                tracing::error!("capture error: {e}");
                return;
            }
        }
    }

    tracing::info!(username, "face recognition exhausted all attempts");
}
