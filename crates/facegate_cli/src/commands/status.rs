use std::path::Path;
use std::process::{Command, Stdio};

use facegate_core::config::Config;
use facegate_core::storage::AuthScope;
use facegate_ipc::{Request, RequestEnvelope, Response, DEFAULT_SOCKET_PATH};
use v4l::video::Capture;
use v4l::Device;

use crate::commands::{broker, session_toggle, sudo_toggle, watch_toggle};

pub fn run(config: &Config, config_path: &Path) -> anyhow::Result<()> {
    println!("Facegate status\n");

    print_config(config, config_path);
    print_broker();
    print_camera(config);
    print_models(config);
    print_templates(config);
    print_audit();
    print_auth();
    print_watch();

    Ok(())
}

pub fn run_streaming(
    config: &Config,
    config_path: &Path,
    tx: &std::sync::mpsc::Sender<String>,
) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    out!("Facegate status");
    out!("");
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
    out!("");
    crate::commands::broker_admin::status_streaming(config, tx)?;
    out!("");
    out!("Auth");
    out!(
        "  sudo   : {}",
        if sudo_toggle::is_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    );
    out!(
        "  session: {}",
        if session_toggle::is_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    );
    out!(
        "  watch  : {}",
        if watch_toggle::is_active() {
            "running"
        } else {
            "stopped"
        }
    );
    Ok(())
}

fn print_audit() {
    println!("Audit");
    let username = current_username();
    match broker::audit_recent(username, 5) {
        Ok(events) if events.is_empty() => println!("  recent : none"),
        Ok(events) => {
            println!("  recent :");
            for event in events {
                println!(
                    "           - {} user={} scope={:?} outcome={:?} reason={:?}",
                    event.timestamp_unix,
                    event.username,
                    event.auth_scope,
                    event.outcome,
                    event.reason
                );
            }
        }
        Err(e) => println!("  recent : unavailable ({e})"),
    }
    println!();
}

fn print_config(config: &Config, config_path: &Path) {
    println!("Config");
    println!("  path   : {}", config_path.display());
    match Config::load(config_path) {
        Ok(_) => println!("  parse  : ok"),
        Err(e) => println!("  parse  : error ({e})"),
    }
    println!("  storage: {}", config.storage.base_dir.display());
    println!();
}

fn print_broker() {
    println!("Broker");
    match facegate_ipc::send_request(DEFAULT_SOCKET_PATH, RequestEnvelope::new(Request::Health)) {
        Ok(response) => match response.response {
            Response::Health { info } => {
                println!(
                    "  socket : ok ({DEFAULT_SOCKET_PATH}, protocol {}, broker {})",
                    info.protocol_version, info.broker_version
                );
            }
            Response::Error(error) => {
                println!("  socket : error ({:?}: {})", error.code, error.message);
            }
            other => println!("  socket : unexpected response ({other:?})"),
        },
        Err(e) => println!("  socket : unavailable ({e})"),
    }
    println!();
}

fn print_camera(config: &Config) {
    println!("Camera");
    let path = Path::new(&config.camera.device);
    println!("  rgb    : {}", config.camera.device);
    println!(
        "           exists={} kind={}",
        yes_no(path.exists()),
        camera_kind(path)
    );
    match config.camera.ir.as_ref() {
        Some(ir) => {
            let ir_path = Path::new(&ir.device);
            println!("  ir     : {}", ir.device);
            println!(
                "           exists={} kind={}",
                yes_no(ir_path.exists()),
                camera_kind(ir_path)
            );
        }
        None => println!("  ir     : not configured"),
    }
    println!(
        "  check  : {}",
        if config.camera.cross_check.enabled {
            "RGB+IR required"
        } else {
            "single camera"
        }
    );
    println!();
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

fn print_models(config: &Config) {
    println!("Models");
    println!(
        "  detector: {} ({})",
        config.models.detector.display(),
        exists_label(&config.models.detector)
    );
    println!(
        "  embedder: {} ({})",
        config.models.embedder.display(),
        exists_label(&config.models.embedder)
    );
    println!();
}

fn print_templates(config: &Config) {
    println!("Templates");
    let users = visible_template_users(config);
    if users.is_empty() {
        println!("  users  : permission-limited or none found");
        if let Some(user) = current_username() {
            print_user_templates(&user);
        }
        println!();
        return;
    }

    for user in users {
        print_user_templates(&user);
    }
    println!();
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

fn print_user_templates(username: &str) {
    match broker::list_templates(username) {
        Ok(templates) if templates.is_empty() => {
            println!("  {username}: none");
        }
        Ok(templates) => {
            let sudo = templates
                .iter()
                .filter(|template| broker::summary_allows(template, AuthScope::Sudo))
                .count();
            let session = templates
                .iter()
                .filter(|template| broker::summary_allows(template, AuthScope::Session))
                .count();
            println!(
                "  {username}: {} total (sudo: {sudo}, session: {session})",
                templates.len()
            );
        }
        Err(e) => {
            println!("  {username}: permission-limited ({e})");
        }
    }
}

fn print_auth() {
    println!("PAM");
    println!("  sudo  : {}", enabled_label(sudo_toggle::is_enabled()));
    let session_entries = session_toggle::enabled_service_entries();
    if session_entries.is_empty() {
        println!("  session: disabled");
    } else {
        println!("  session: enabled");
        for (name, service, path) in session_entries {
            println!("           - {name} ({service}): {path}");
        }
    }
    println!();
}

fn print_watch() {
    println!("Watch daemon");
    println!(
        "  unit   : {}",
        if watch_toggle::is_installed() {
            "installed"
        } else {
            "missing"
        }
    );
    match systemctl_user(["is-enabled", "facegate-watch"]) {
        Some(output) => println!("  enabled: {}", output.trim()),
        None => println!("  enabled: unavailable"),
    }
    match systemctl_user(["is-active", "facegate-watch"]) {
        Some(output) => println!("  active : {}", output.trim()),
        None => println!("  active : unavailable"),
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
