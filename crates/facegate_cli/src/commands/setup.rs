use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::{bail, Context as _};
use facegate_core::config::Config;
use v4l::video::Capture;
use v4l::Device;

use crate::commands::{add, session_toggle, sudo_toggle, test, watch_toggle};

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

    if let Some(camera) = recommended_camera() {
        println!("Recommended camera: {camera}");
        if ask_yes_no("Write this camera to the config?", true)? {
            backup_config(&config_path)?;
            config.camera.device = camera;
            write_config(&config_path, &config)?;
            println!("Config updated.");
        }
    } else {
        println!("No usable /dev/video* camera recommendation found.");
        println!("Run `facegate cameras` later if capture fails.");
    }

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
    println!("Setup finished. Useful next commands:");
    println!("  facegate status");
    println!("  sudo facegate doctor");
    println!("  sudo facegate test {username}");
    Ok(())
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

fn recommended_camera() -> Option<String> {
    let mut paths = std::fs::read_dir("/dev")
        .ok()?
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

    let mut rgb = None;
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
        if is_ir {
            return Some(path.display().to_string());
        }
        if is_rgb && rgb.is_none() {
            rgb = Some(path.display().to_string());
        }
    }
    rgb
}
