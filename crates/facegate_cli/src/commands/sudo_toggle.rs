use std::sync::mpsc::Sender;

const PAM_SUDO: &str = "/etc/pam.d/sudo";
const PAM_LINE: &str = "auth      sufficient    pam_facegate.so";

pub fn is_enabled() -> bool {
    std::fs::read_to_string(PAM_SUDO)
        .map(|c| c.lines().any(|l| l.trim() == PAM_LINE.trim()))
        .unwrap_or(false)
}

pub fn run_streaming(_username: Option<&str>, tx: &Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    let content = match std::fs::read_to_string(PAM_SUDO) {
        Ok(c) => c,
        Err(e) => {
            anyhow::bail!("cannot read {PAM_SUDO}: {e}");
        }
    };

    let already_enabled = content.lines().any(|l| l.trim() == PAM_LINE.trim());

    if already_enabled {
        let new_content: String = content
            .lines()
            .filter(|l| l.trim() != PAM_LINE.trim())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(PAM_SUDO, &new_content)
            .map_err(|e| anyhow::anyhow!("cannot write {PAM_SUDO}: {e} (run as root?)"))?;
        out!("Sudo face authentication disabled.");
        out!("");
        out!("Removed from {PAM_SUDO}:");
        out!("  {PAM_LINE}");
    } else {
        // Insert after the first line (usually #%PAM-1.0) so it runs before
        // the default password auth.
        let mut lines: Vec<&str> = content.lines().collect();
        let insert_at = if lines.first().map(|l| l.starts_with('#')).unwrap_or(false) {
            1
        } else {
            0
        };
        lines.insert(insert_at, PAM_LINE);
        let new_content = lines.join("\n") + "\n";
        std::fs::write(PAM_SUDO, &new_content)
            .map_err(|e| anyhow::anyhow!("cannot write {PAM_SUDO}: {e} (run as root?)"))?;
        out!("Sudo face authentication enabled.");
        out!("");
        out!("Added to {PAM_SUDO}:");
        out!("  {PAM_LINE}");
        out!("");
        out!("sudo will now try face recognition first, then fall back to password.");
    }

    Ok(())
}
