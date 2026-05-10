use std::sync::mpsc::Sender;

use crate::commands::pam_edit::{self, PAM_LINE};

const SESSION_PAM_SERVICES: &[(&str, &str)] = &[
    ("console login", "login"),
    ("GDM", "gdm-password"),
    ("GDM3 (Ubuntu/Debian)", "gdm3"),
    ("GDM (generic)", "gdm"),
    ("SDDM", "sddm"),
    ("LightDM", "lightdm"),
    ("greetd", "greetd"),
    ("KDE / Plasma lock screen", "kde"),
    ("vlock", "vlock"),
    ("GNOME screensaver", "gnome-screensaver"),
    ("swaylock", "swaylock"),
    ("hyprlock", "hyprlock"),
    ("i3lock", "i3lock"),
];

/// Returns the list of services that currently have the facegate PAM line,
/// with their display name and actual file path.
pub fn enabled_service_entries() -> Vec<(&'static str, &'static str, String)> {
    SESSION_PAM_SERVICES
        .iter()
        .copied()
        .filter(|(_, service)| pam_edit::service_has_line(service))
        .filter_map(|(name, service)| {
            pam_edit::service_path(service).map(|path| (name, service, path))
        })
        .collect()
}

pub fn is_enabled() -> bool {
    let services = existing_services();
    !services.is_empty()
        && services
            .iter()
            .all(|(_, service)| pam_edit::service_has_line(service))
}

pub fn run(extra_services: &[&str], extra_files: &[&str]) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    run_streaming(None, extra_services, extra_files, &tx)?;
    drop(tx);
    for line in rx {
        println!("{line}");
    }
    Ok(())
}

pub fn run_streaming(
    _username: Option<&str>,
    extra_services: &[&str],
    extra_files: &[&str],
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    let known = existing_services();
    if known.is_empty() && extra_services.is_empty() && extra_files.is_empty() {
        anyhow::bail!("no supported login PAM service found under /etc/pam.d or /usr/lib/pam.d;\
            \nuse --pam-service <name> or --pam-file <path> to specify one manually");
    }

    let all_enabled = known.iter().all(|(_, s)| pam_edit::service_has_line(s))
        && extra_services.iter().all(|s| pam_edit::service_has_line(s))
        && extra_files.iter().all(|f| pam_edit::file_has_line(f));
    let enable = !all_enabled;

    let mut changed: Vec<String> = Vec::new();
    for (name, service) in &known {
        if pam_edit::set_service_enabled(service, enable)? {
            changed.push(format!("  {name}: /etc/pam.d/{service}"));
        }
    }
    for service in extra_services {
        if pam_edit::set_service_enabled(service, enable)? {
            changed.push(format!("  {service}"));
        }
    }
    for file in extra_files {
        if pam_edit::set_enabled(file, enable)? {
            changed.push(format!("  {file}"));
        }
    }

    if enable {
        out!("Session face authentication enabled.");
        out!("");
        out!("Added to supported login PAM services:");
    } else {
        out!("Session face authentication disabled.");
        out!("");
        out!("Removed from supported login PAM services:");
    }
    for line in changed {
        out!("{line}");
    }
    out!("");
    out!("PAM line:");
    out!("  {PAM_LINE}");
    out!("");
    out!("Keep a root shell open while testing login/session authentication.");

    Ok(())
}

fn existing_services() -> Vec<(&'static str, &'static str)> {
    SESSION_PAM_SERVICES
        .iter()
        .copied()
        .filter(|(_, service)| pam_edit::service_exists(service))
        .collect()
}
