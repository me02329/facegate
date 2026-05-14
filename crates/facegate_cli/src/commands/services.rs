use std::process::{Command, Stdio};

use crate::commands::{user_log, watch_toggle};

#[derive(Debug, Clone)]
pub struct RefreshSummary {
    pub broker: RefreshState,
    pub watch: RefreshState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshState {
    Restarted,
    NotRunning,
    SystemdUnavailable,
    Failed,
}

impl RefreshState {
    pub fn label(self) -> &'static str {
        match self {
            RefreshState::Restarted => "restarted",
            RefreshState::NotRunning => "not running",
            RefreshState::SystemdUnavailable => "systemd unavailable",
            RefreshState::Failed => "failed",
        }
    }
}

pub fn refresh_after_config_change() -> RefreshSummary {
    let broker = restart_broker_if_available();
    let watch = watch_toggle::restart_if_active();
    user_log::append_for_current_or_sudo_user(format!(
        "services refresh_after_config_change broker={} watch={}",
        broker.label(),
        watch.label()
    ));
    RefreshSummary { broker, watch }
}

pub fn print_refresh_summary(summary: &RefreshSummary) {
    println!(
        "Services refreshed: broker={}, watch={}",
        summary.broker.label(),
        summary.watch.label()
    );
}

fn restart_broker_if_available() -> RefreshState {
    if !std::path::Path::new("/run/systemd/system").exists() {
        return RefreshState::SystemdUnavailable;
    }
    let active = is_broker_active();
    let action = if active { "restart" } else { "start" };
    let ok = Command::new("systemctl")
        .arg(action)
        .arg("facegate-brokerd.service")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if ok {
        RefreshState::Restarted
    } else {
        RefreshState::Failed
    }
}

/// `true` if `facegate-brokerd.service` is currently active. Returns false if
/// systemd is not available.
pub fn is_broker_active() -> bool {
    if !std::path::Path::new("/run/systemd/system").exists() {
        return false;
    }
    Command::new("systemctl")
        .arg("is-active")
        .arg("--quiet")
        .arg("facegate-brokerd.service")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// `true` if `facegate-brokerd.service` is enabled to start at boot.
pub fn is_broker_enabled() -> bool {
    if !std::path::Path::new("/run/systemd/system").exists() {
        return false;
    }
    Command::new("systemctl")
        .arg("is-enabled")
        .arg("--quiet")
        .arg("facegate-brokerd.service")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Make sure the broker is enabled at boot and running now. No-op when
/// systemd is unavailable. Returns `Ok(true)` if the broker ended up active.
pub fn ensure_broker_active() -> std::io::Result<bool> {
    if !std::path::Path::new("/run/systemd/system").exists() {
        return Ok(false);
    }
    if !is_broker_enabled() {
        let _ = Command::new("systemctl")
            .arg("enable")
            .arg("facegate-brokerd.service")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
    }
    if !is_broker_active() {
        let _ = Command::new("systemctl")
            .arg("start")
            .arg("facegate-brokerd.service")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
    }
    Ok(is_broker_active())
}
