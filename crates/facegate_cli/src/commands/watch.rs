use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context as _;
use futures_util::StreamExt as _;
use zbus::Connection;
use zbus::proxy;

use facegate_core::config::Config;
use facegate_core::error::FaceRsError;
use facegate_core::matching::is_match;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::{AuthScope, TemplateStore};

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
    let username = std::env::var("USER").context(
        "USER env var not set — facegate-watch must run inside a user session",
    )?;
    let session_id = std::env::var("XDG_SESSION_ID").context(
        "XDG_SESSION_ID not set — facegate-watch must run inside a user session",
    )?;

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

    loop {
        tokio::select! {
            biased;

            // Graceful shutdown on SIGTERM or Ctrl-C.
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received shutdown signal");
                if let Some(cancel) = scan_cancel.take() {
                    cancel.store(true, Ordering::Relaxed);
                }
                break;
            }

            Some(_) = lock_stream.next() => {
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

            Some(_) = unlock_stream.next() => {
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
    let store = TemplateStore::new(&config.storage.base_dir);
    let enrolled = match store.embeddings_for_scope(username, AuthScope::Session) {
        Ok(e) if !e.is_empty() => e,
        Ok(_) | Err(FaceRsError::NotEnrolled) => {
            tracing::warn!(username, "no session templates — skipping face scan");
            return;
        }
        Err(e) => {
            tracing::error!(username, "template load error: {e}");
            return;
        }
    };

    if cancel.load(Ordering::Relaxed) {
        return;
    }

    let mut pipeline = match FacePipeline::new(config) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("cannot open camera: {e}");
            return;
        }
    };

    let threshold = config.recognition.threshold;

    for attempt in 1..=config.recognition.max_attempts {
        if cancel.load(Ordering::Relaxed) {
            tracing::debug!("recognition cancelled (attempt {attempt})");
            return;
        }

        match pipeline.capture_embedding(config) {
            Ok(embedding) => {
                if is_match(&embedding, &enrolled, threshold) {
                    tracing::info!(username, "face recognised — unlocking session");
                    // Use the async D-Bus proxy from a blocking context.
                    let result = tokio::runtime::Handle::current()
                        .block_on(session.unlock());
                    if let Err(e) = result {
                        tracing::error!("unlock call failed: {e}");
                    }
                    return;
                }
                tracing::debug!(attempt, username, "attempt did not match");
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
