use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};

use facegate_core::config::Config;
use facegate_core::storage::AuthScope;
use v4l::video::Capture;
use v4l::Device;

use crate::commands::{broker, session_toggle, sudo_toggle, watch_toggle};

pub fn run(config: &Config, config_path: &Path) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<String>();
    let config = config.clone();
    let config_path = config_path.to_path_buf();
    let handle = std::thread::spawn(move || {
        let result = run_streaming(&config, &config_path, &tx);
        drop(tx);
        result
    });
    while let Ok(line) = rx.recv() {
        println!("{line}");
    }
    handle.join().unwrap()
}

pub fn run_streaming(
    config: &Config,
    config_path: &Path,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    let _ = tx.send("Facegate status".to_owned());
    let _ = tx.send(String::new());
    emit_config(config, config_path, tx);
    let _ = tx.send(String::new());
    crate::commands::broker_admin::status_streaming(config, tx)?;
    let _ = tx.send(String::new());
    emit_camera(config, tx);
    let _ = tx.send(String::new());
    emit_models(config, tx);
    let _ = tx.send(String::new());
    emit_templates(config, tx);
    let _ = tx.send(String::new());
    emit_audit(tx);
    let _ = tx.send(String::new());
    emit_auth(tx);
    let _ = tx.send(String::new());
    emit_watch(tx);
    Ok(())
}

fn emit_audit(tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    out!("Audit");
    let username = current_username();
    match broker::audit_recent(username, 5) {
        Ok(events) if events.is_empty() => out!("  recent : none"),
        Ok(events) => {
            out!("  recent :");
            for event in events {
                out!(
                    "           - {} user={} scope={} outcome={} reason={}",
                    event.timestamp_unix,
                    event.username,
                    audit_scope_label(&event.auth_scope),
                    audit_outcome_label(&event.outcome),
                    audit_reason_label(&event.reason),
                );
            }
        }
        Err(e) => out!("  recent : unavailable ({e})"),
    }
}

fn emit_config(config: &Config, config_path: &Path, tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    out!("Config");
    out!("  path   : {}", config_path.display());
    out!(
        "  parse  : {}",
        match Config::load(config_path) {
            Ok(_) => "ok".to_owned(),
            Err(e) => format!("error ({e})"),
        }
    );
    out!("  storage: {}", config.storage.base_dir.display());
}

fn emit_camera(config: &Config, tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    out!("Camera");
    let path = Path::new(&config.camera.device);
    out!("  rgb    : {}", config.camera.device);
    out!(
        "           exists={} kind={}",
        yes_no(path.exists()),
        camera_kind(path)
    );
    match config.camera.ir.as_ref() {
        Some(ir) => {
            let ir_path = Path::new(&ir.device);
            out!("  ir     : {}", ir.device);
            out!(
                "           exists={} kind={}",
                yes_no(ir_path.exists()),
                camera_kind(ir_path)
            );
        }
        None => out!("  ir     : not configured"),
    }
    out!(
        "  check  : {}",
        if config.camera.cross_check.enabled {
            "RGB+IR required"
        } else {
            "single camera"
        }
    );
}

fn camera_kind(path: &Path) -> &'static str {
    let Ok(dev) = Device::with_path(path) else {
        return "unknown";
    };
    let Ok(formats) = dev.enum_formats() else {
        return "unknown";
    };
    let has_ir = formats
        .iter()
        .any(|f| matches!(f.fourcc.to_string().as_str(), "GREY" | "Y8  " | "Y800"));
    let has_rgb = formats
        .iter()
        .any(|f| matches!(f.fourcc.to_string().as_str(), "YUYV" | "MJPG"));
    match (has_ir, has_rgb) {
        (true, _) => "IR",
        (false, true) => "RGB",
        (false, false) => "unknown",
    }
}

fn emit_models(config: &Config, tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    out!("Models");
    out!(
        "  detector: {} ({})",
        config.models.detector.display(),
        exists_label(&config.models.detector)
    );
    out!(
        "  embedder: {} ({})",
        config.models.embedder.display(),
        exists_label(&config.models.embedder)
    );
}

fn emit_templates(config: &Config, tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    out!("Templates");
    let users = visible_template_users(config);
    if users.is_empty() {
        out!("  users  : permission-limited or none found");
        if let Some(user) = current_username() {
            emit_user_templates(&user, tx);
        }
        return;
    }
    for user in users {
        emit_user_templates(&user, tx);
    }
}

fn visible_template_users(config: &Config) -> Vec<String> {
    if unsafe { libc::geteuid() } != 0 {
        return current_username().into_iter().collect();
    }
    let Ok(entries) = std::fs::read_dir(&config.storage.base_dir) else {
        return current_username().into_iter().collect();
    };
    let mut users = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|ft| ft.is_dir())
                .and_then(|_| entry.file_name().into_string().ok())
        })
        .collect::<Vec<_>>();
    users.sort();
    users
}

fn emit_user_templates(username: &str, tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    match broker::list_templates(username) {
        Ok(templates) if templates.is_empty() => out!("  {username}: none"),
        Ok(templates) => {
            let sudo = templates
                .iter()
                .filter(|template| broker::summary_allows(template, AuthScope::Sudo))
                .count();
            let session = templates
                .iter()
                .filter(|template| broker::summary_allows(template, AuthScope::Session))
                .count();
            out!(
                "  {username}: {} total (sudo: {sudo}, session: {session})",
                templates.len()
            );
        }
        Err(e) => out!("  {username}: permission-limited ({e})"),
    }
}

fn emit_auth(tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    out!("PAM");
    out!("  sudo  : {}", enabled_label(sudo_toggle::is_enabled()));
    let session_entries = session_toggle::enabled_service_entries();
    if session_entries.is_empty() {
        out!("  session: disabled");
    } else {
        out!("  session: enabled");
        for (name, service, path) in session_entries {
            out!("           - {name} ({service}): {path}");
        }
    }
}

fn emit_watch(tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    out!("Watch daemon");
    out!(
        "  unit   : {}",
        if watch_toggle::is_installed() {
            "installed"
        } else {
            "missing"
        }
    );
    match systemctl_user(["is-enabled", "facegate-watch"]) {
        Some(output) => out!("  enabled: {}", output.trim()),
        None => out!("  enabled: unavailable"),
    }
    match systemctl_user(["is-active", "facegate-watch"]) {
        Some(output) => out!("  active : {}", output.trim()),
        None => out!("  active : unavailable"),
    }
}

fn audit_scope_label(scope: &facegate_ipc::AuthScope) -> &'static str {
    match scope {
        facegate_ipc::AuthScope::Sudo => "sudo",
        facegate_ipc::AuthScope::Session => "session",
    }
}

fn audit_outcome_label(outcome: &facegate_ipc::AuditOutcome) -> &'static str {
    match outcome {
        facegate_ipc::AuditOutcome::Success => "success",
        facegate_ipc::AuditOutcome::Failure => "failure",
    }
}

fn audit_reason_label(reason: &facegate_ipc::AuditReason) -> &'static str {
    match reason {
        facegate_ipc::AuditReason::Matched => "matched",
        facegate_ipc::AuditReason::Mismatch => "mismatch",
        facegate_ipc::AuditReason::NotEnrolled => "not_enrolled",
        facegate_ipc::AuditReason::RateLimited => "rate_limited",
        facegate_ipc::AuditReason::LockedOut => "locked_out",
        facegate_ipc::AuditReason::Unauthorized => "unauthorized",
        facegate_ipc::AuditReason::Internal => "internal",
    }
}

fn systemctl_user<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if output.stdout.is_empty() && !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    Some(text.lines().next().unwrap_or("").to_owned())
}

fn current_username() -> Option<String> {
    std::env::var("SUDO_USER")
        .ok()
        .filter(|s| !s.is_empty() && s != "root")
        .or_else(|| std::env::var("USER").ok().filter(|s| !s.is_empty()))
}

fn exists_label(path: &Path) -> &'static str {
    if path.exists() {
        "present"
    } else {
        "missing"
    }
}

fn enabled_label(enabled: bool) -> &'static str {
    if enabled {
        "enabled"
    } else {
        "disabled"
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
