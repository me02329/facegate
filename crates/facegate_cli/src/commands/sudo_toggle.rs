use std::sync::mpsc::Sender;
use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path};

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
        backup_pam_file()?;
        write_pam_atomic(&new_content)?;
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
        backup_pam_file()?;
        write_pam_atomic(&new_content)?;
        out!("Sudo face authentication enabled.");
        out!("");
        out!("Added to {PAM_SUDO}:");
        out!("  {PAM_LINE}");
        out!("");
        out!("sudo will now try face recognition first, then fall back to password.");
    }

    Ok(())
}

fn backup_pam_file() -> anyhow::Result<()> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = format!("{PAM_SUDO}.facegate.{secs}.bak");
    fs::copy(PAM_SUDO, &backup)
        .map_err(|e| anyhow::anyhow!("cannot create PAM backup {backup}: {e}"))?;
    Ok(())
}

fn write_pam_atomic(new_content: &str) -> anyhow::Result<()> {
    let path = Path::new(PAM_SUDO);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{PAM_SUDO} has no parent directory"))?;
    let meta = fs::metadata(path).map_err(|e| anyhow::anyhow!("cannot inspect {PAM_SUDO}: {e}"))?;
    let mode = meta.permissions().mode() & 0o777;
    let tmp_path = parent.join(format!(".sudo.facegate.{}.tmp", std::process::id()));

    let write_result = (|| -> anyhow::Result<()> {
        let mut tmp = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", tmp_path.display()))?;
        tmp.write_all(new_content.as_bytes())
            .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", tmp_path.display()))?;
        tmp.sync_all()
            .map_err(|e| anyhow::anyhow!("cannot sync {}: {e}", tmp_path.display()))?;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode))
            .map_err(|e| anyhow::anyhow!("cannot chmod {}: {e}", tmp_path.display()))?;
        fs::rename(&tmp_path, path).map_err(|e| {
            anyhow::anyhow!("cannot replace {PAM_SUDO} with {}: {e}", tmp_path.display())
        })?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    write_result
}
