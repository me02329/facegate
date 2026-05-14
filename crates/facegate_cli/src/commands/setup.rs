use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::{bail, Context as _};
use facegate_core::config::{CameraIrConfig, Config};
use v4l::video::Capture;
use v4l::Device;

use crate::commands::services;
use crate::commands::{add, calibrate_cameras, session_toggle, sudo_toggle, test, watch_toggle};

pub fn run(
    mut config: Config,
    config_path: PathBuf,
    username: Option<String>,
) -> anyhow::Result<()> {
    let username = username
        .or_else(default_username)
        .ok_or_else(|| anyhow::anyhow!("cannot determine user to enroll; pass USERNAME"))?;

    println!("Facegate guided setup\n");
    println!("User       : {username}");
    println!("Config path: {}", config_path.display());
    println!();

    // 1. Make sure the broker is reachable before we do anything else — every
    //    enrollment / test path goes through it.
    ensure_broker_or_warn();

    // 2. Pick and write the RGB primary camera + optionally the IR sensor.
    config = configure_cameras(config, &config_path)?;

    if ask_yes_no("Enroll face templates now?", true)? {
        add::run(&config, &username, None, add::EnrollmentTarget::Both)?;
    } else {
        println!("Skipped enrollment.");
    }

    if ask_yes_no("Run a recognition test now?", true)? {
        test::run(&config, &username, test::TestScope::All)?;
    } else {
        println!("Skipped recognition test.");
    }

    if !sudo_toggle::is_enabled() && ask_yes_no("Enable sudo face authentication?", true)? {
        stream_command(|tx| sudo_toggle::run_streaming(Some(&username), tx))?;
    }

    if !session_toggle::is_enabled()
        && ask_yes_no("Enable login/session face authentication?", true)?
    {
        stream_command(|tx| session_toggle::run_streaming(Some(&username), &[], &[], tx))?;
    }

    if watch_toggle::is_installed() && ask_yes_no("Enable screen-lock auto-unlock daemon?", true)? {
        stream_command(|tx| watch_toggle::run_streaming(true, tx))?;
    }

    println!();
    print_services_summary();
    println!();
    println!("Setup finished. Useful next commands:");
    println!("  facegate status");
    println!("  sudo facegate doctor");
    println!("  sudo facegate test {username}");
    Ok(())
}

/// Try to make sure `facegate-brokerd` is enabled + running. Doesn't bail on
/// failure — enrolment will surface a clearer error if the broker is really
/// missing — but warns the user so they don't have to read systemd logs to
/// figure out why later steps fail.
fn ensure_broker_or_warn() {
    match services::ensure_broker_active() {
        Ok(true) => {
            if !services::is_broker_enabled() {
                println!("Note: broker is running but not enabled at boot.");
            }
        }
        Ok(false) => {
            println!(
                "Warning: facegate-brokerd is not running and could not be started.\n\
                 Run `sudo systemctl enable --now facegate-brokerd.service` and re-run setup."
            );
        }
        Err(e) => {
            println!("Warning: cannot reach systemd to start the broker: {e}");
        }
    }
}

/// Detect which `/dev/videoN` nodes are RGB vs IR and offer to write them to
/// the config. Returns the (possibly updated) config; the on-disk file may
/// have been rewritten and the broker restarted.
fn configure_cameras(mut config: Config, config_path: &Path) -> anyhow::Result<Config> {
    let cams = recommend_cameras();

    match (&cams.rgb, &cams.ir) {
        (None, None) => {
            println!("No usable /dev/video* camera found.");
            println!("Run `facegate cameras` to investigate, then `sudo facegate configure`.");
            return Ok(config);
        }
        (Some(rgb), None) => {
            println!("Recommended camera (RGB): {rgb}");
            println!("No IR sensor detected — cross-check unavailable on this hardware.");
            if ask_yes_no("Write this camera to the config?", true)? {
                backup_config(config_path)?;
                config.camera.device = rgb.clone();
                // Drop any stale IR / cross-check that no longer makes sense.
                if config.camera.ir.is_some() {
                    config.camera.ir = None;
                    config.camera.cross_check.enabled = false;
                }
                write_config(config_path, &config)?;
                println!("Config updated.");
                services::print_refresh_summary(&services::refresh_after_config_change());
            }
            return Ok(config);
        }
        (None, Some(ir)) => {
            println!("Only an IR sensor was detected ({ir}); facegate currently needs an RGB camera as the primary.");
            println!("Plug in a webcam and re-run setup, or set [camera].device manually.");
            return Ok(config);
        }
        (Some(_), Some(_)) => {}
    }

    let rgb = cams.rgb.as_ref().unwrap();
    let ir = cams.ir.as_ref().unwrap();

    println!("Detected cameras:");
    println!("  RGB : {rgb}");
    println!("  IR  : {ir}");
    println!();

    if !ask_yes_no("Write these to the config?", true)? {
        println!("Config unchanged.");
        return Ok(config);
    }

    backup_config(config_path)?;
    config.camera.device = rgb.clone();

    let setup_cross_check = ask_yes_no(
        "Configure RGB+IR cross-check (Windows-Hello-style liveness)?",
        true,
    )?;
    if setup_cross_check {
        // Preserve any IR-specific overrides the user might already have.
        match config.camera.ir.as_mut() {
            Some(ir_cfg) => ir_cfg.device = ir.clone(),
            None => {
                config.camera.ir = Some(CameraIrConfig {
                    device: ir.clone(),
                    width: None,
                    height: None,
                    fps: None,
                    timeout_ms: None,
                    warmup_frames: None,
                    min_face_size: None,
                });
            }
        }
        // Don't enable cross-check yet — the homography is still identity.
        // calibrate_cameras --write --enable will flip it on once calibrated.
        config.camera.cross_check.enabled = false;
    } else {
        // User declined cross-check: keep things in a single-camera state.
        config.camera.ir = None;
        config.camera.cross_check.enabled = false;
    }
    write_config(config_path, &config)?;
    println!("Config updated.");
    services::print_refresh_summary(&services::refresh_after_config_change());

    if setup_cross_check
        && ask_yes_no(
            "Calibrate RGB+IR alignment now? (required for cross-check)",
            true,
        )?
    {
        println!();
        calibrate_cameras::run(
            config.clone(),
            config_path.to_path_buf(),
            Some(rgb),
            Some(ir),
            3,
            true, // write
            true, // enable cross-check on success
        )?;
        // Calibration rewrote the config on disk; reload so the rest of setup
        // (enroll / test) sees the freshly-enabled cross-check.
        if let Ok(reloaded) = Config::load(config_path) {
            config = reloaded;
        }
    }

    Ok(config)
}

fn print_services_summary() {
    println!("Services summary");
    println!(
        "  facegate-brokerd : {}",
        match (services::is_broker_active(), services::is_broker_enabled()) {
            (true, true) => "running, enabled at boot",
            (true, false) => "running, NOT enabled at boot",
            (false, true) => "stopped, enabled at boot",
            (false, false) => "stopped, not enabled",
        }
    );
    println!(
        "  facegate-watch   : {}",
        if !watch_toggle::is_installed() {
            "not installed"
        } else if watch_toggle::is_active() {
            "running"
        } else {
            "installed, not running"
        }
    );
    println!(
        "  sudo PAM         : {}",
        if sudo_toggle::is_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!(
        "  session PAM      : {}",
        if session_toggle::is_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    );
}

fn stream_command<F>(f: F) -> anyhow::Result<()>
where
    F: FnOnce(&mpsc::Sender<String>) -> anyhow::Result<()>,
{
    let (tx, rx) = mpsc::channel();
    f(&tx)?;
    drop(tx);
    for line in rx {
        println!("{line}");
    }
    Ok(())
}

fn default_username() -> Option<String> {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|s| !s.is_empty() && s != "root")
        .or_else(|| std::env::var("USER").ok().filter(|s| !s.is_empty()))
}

fn ask_yes_no(prompt: &str, default_yes: bool) -> anyhow::Result<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{prompt} {suffix} ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let trimmed = line.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Ok(default_yes);
    }
    match trimmed.as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => bail!("expected yes or no"),
    }
}

fn backup_config(config_path: &Path) -> anyhow::Result<()> {
    if !config_path.exists() {
        return Ok(());
    }
    let backup = config_path.with_extension(format!(
        "{}.bak",
        config_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("toml")
    ));
    std::fs::copy(config_path, &backup).with_context(|| {
        format!(
            "cannot back up {} to {}",
            config_path.display(),
            backup.display()
        )
    })?;
    println!("Backed up config to {}", backup.display());
    Ok(())
}

fn write_config(config_path: &Path, config: &Config) -> anyhow::Result<()> {
    let parent = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", config_path.display()))?;
    std::fs::create_dir_all(parent)?;
    let toml = toml::to_string_pretty(config)?;
    std::fs::write(config_path, toml)?;
    Ok(())
}

#[derive(Debug, Default, Clone)]
struct RecommendedCameras {
    rgb: Option<String>,
    ir: Option<String>,
}

/// Scan `/dev/video*` and return the first RGB and first IR device found, if
/// any. Unlike the previous heuristic, this never returns an IR device as the
/// "primary camera" — RGB is always the primary for facegate v0.3+, and IR
/// lives in its own `[camera.ir]` section.
fn recommend_cameras() -> RecommendedCameras {
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return RecommendedCameras::default();
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| {
                    name.starts_with("video") && name[5..].chars().all(|c| c.is_ascii_digit())
                })
        })
        .collect::<Vec<_>>();
    paths.sort();

    let mut out = RecommendedCameras::default();
    for path in paths {
        let Ok(dev) = Device::with_path(&path) else {
            continue;
        };
        let Ok(formats) = dev.enum_formats() else {
            continue;
        };
        let is_ir = formats
            .iter()
            .any(|f| matches!(f.fourcc.to_string().as_str(), "GREY" | "Y8  " | "Y800"));
        let is_rgb = formats
            .iter()
            .any(|f| matches!(f.fourcc.to_string().as_str(), "YUYV" | "MJPG"));
        // GREY-only nodes are IR sensors; nodes that also expose YUYV/MJPG are
        // colour cameras (some IR modules report both, but they're rare and
        // the RGB stream is usable, so we treat them as RGB).
        if is_ir && !is_rgb && out.ir.is_none() {
            out.ir = Some(path.display().to_string());
        } else if is_rgb && out.rgb.is_none() {
            out.rgb = Some(path.display().to_string());
        }
    }
    out
}
