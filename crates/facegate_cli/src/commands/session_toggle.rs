use std::path::Path;
use std::sync::mpsc::Sender;

use crate::commands::pam_edit::{self, PAM_LINE};

const SESSION_PAM_SERVICES: &[(&str, &str)] = &[
    ("console login", "/etc/pam.d/login"),
    ("GDM", "/etc/pam.d/gdm-password"),
    ("SDDM", "/etc/pam.d/sddm"),
    ("LightDM", "/etc/pam.d/lightdm"),
    ("greetd", "/etc/pam.d/greetd"),
    ("KDE", "/etc/pam.d/kde"),
];

pub fn is_enabled() -> bool {
    existing_services()
        .iter()
        .any(|(_, path)| pam_edit::file_has_line(path))
}

pub fn run() -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    run_streaming(None, &tx)?;
    drop(tx);
    for line in rx {
        println!("{line}");
    }
    Ok(())
}

pub fn run_streaming(_username: Option<&str>, tx: &Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    let services = existing_services();
    if services.is_empty() {
        anyhow::bail!("no supported login PAM service found under /etc/pam.d");
    }

    let already_enabled = services
        .iter()
        .any(|(_, path)| pam_edit::file_has_line(path));
    let enable = !already_enabled;

    let mut changed = Vec::new();
    for (name, path) in services {
        if pam_edit::set_enabled(path, enable)? {
            changed.push((name, path));
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
    for (name, path) in changed {
        out!("  {name}: {path}");
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
        .filter(|(_, path)| Path::new(path).exists())
        .collect()
}
