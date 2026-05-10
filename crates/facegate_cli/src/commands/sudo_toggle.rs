use std::path::Path;
use std::sync::mpsc::Sender;

use crate::commands::pam_edit::{self, PAM_LINE};

const PAM_SUDO: &str = "/etc/pam.d/sudo";
const PAM_SUDO_I: &str = "/etc/pam.d/sudo-i";

/// Returns the list of sudo PAM files that exist on this system.
/// `sudo-i` is missing on a few distros; we silently skip it then.
fn existing_sudo_files() -> Vec<&'static str> {
    [PAM_SUDO, PAM_SUDO_I]
        .into_iter()
        .filter(|p| Path::new(p).exists())
        .collect()
}

pub fn is_enabled() -> bool {
    let files = existing_sudo_files();
    if files.is_empty() {
        return false;
    }
    files.iter().all(|p| pam_edit::file_has_line(p))
}

pub fn run_streaming(_username: Option<&str>, tx: &Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    let files = existing_sudo_files();
    if files.is_empty() {
        anyhow::bail!("no sudo PAM service file found at /etc/pam.d/sudo[-i]");
    }

    // We toggle to whatever state is *not* fully enabled, so a partial state
    // (e.g. sudo enabled but sudo-i not) gets normalised by enabling all.
    let all_enabled = files.iter().all(|p| pam_edit::file_has_line(p));
    let enable = !all_enabled;

    let mut changed: Vec<&str> = Vec::new();
    for path in &files {
        if pam_edit::set_enabled(path, enable)? {
            changed.push(path);
        }
    }

    if enable {
        out!("Sudo face authentication enabled.");
        out!("");
        out!("Added to:");
    } else {
        out!("Sudo face authentication disabled.");
        out!("");
        out!("Removed from:");
    }
    for path in &files {
        let mark = if changed.contains(path) { "✓" } else { "·" };
        out!("  [{mark}] {path}");
    }
    out!("");
    out!("PAM line:");
    out!("  {PAM_LINE}");
    if enable {
        out!("");
        out!("sudo will now try face recognition first, then fall back to password.");
    }

    Ok(())
}
