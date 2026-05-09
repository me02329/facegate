use std::sync::mpsc::Sender;

use crate::commands::pam_edit::{self, PAM_LINE};

const PAM_SUDO: &str = "/etc/pam.d/sudo";

pub fn is_enabled() -> bool {
    pam_edit::file_has_line(PAM_SUDO)
}

pub fn run_streaming(_username: Option<&str>, tx: &Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    let already_enabled = is_enabled();

    if already_enabled {
        pam_edit::set_enabled(PAM_SUDO, false)?;
        out!("Sudo face authentication disabled.");
        out!("");
        out!("Removed from {PAM_SUDO}:");
        out!("  {PAM_LINE}");
    } else {
        pam_edit::set_enabled(PAM_SUDO, true)?;
        out!("Sudo face authentication enabled.");
        out!("");
        out!("Added to {PAM_SUDO}:");
        out!("  {PAM_LINE}");
        out!("");
        out!("sudo will now try face recognition first, then fall back to password.");
    }

    Ok(())
}
