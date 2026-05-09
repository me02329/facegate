use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path};

pub const PAM_LINE: &str = "auth      sufficient    pam_facegate.so";

pub fn file_has_line(path: &str) -> bool {
    fs::read_to_string(path)
        .map(|c| c.lines().any(|l| l.trim() == PAM_LINE.trim()))
        .unwrap_or(false)
}

pub fn set_enabled(path: &str, enabled: bool) -> anyhow::Result<bool> {
    let content =
        fs::read_to_string(path).map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    let already_enabled = content.lines().any(|l| l.trim() == PAM_LINE.trim());

    if enabled == already_enabled {
        return Ok(false);
    }

    let new_content = if enabled {
        let mut lines: Vec<&str> = content.lines().collect();
        let insert_at = if lines.first().map(|l| l.starts_with('#')).unwrap_or(false) {
            1
        } else {
            0
        };
        lines.insert(insert_at, PAM_LINE);
        lines.join("\n") + "\n"
    } else {
        content
            .lines()
            .filter(|l| l.trim() != PAM_LINE.trim())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    };

    backup_pam_file(path)?;
    write_pam_atomic(path, &new_content)?;
    Ok(true)
}

fn backup_pam_file(path: &str) -> anyhow::Result<()> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = format!("{path}.facegate.{secs}.bak");
    fs::copy(path, &backup)
        .map_err(|e| anyhow::anyhow!("cannot create PAM backup {backup}: {e}"))?;
    Ok(())
}

fn write_pam_atomic(path: &str, new_content: &str) -> anyhow::Result<()> {
    let path_ref = Path::new(path);
    let parent = path_ref
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{path} has no parent directory"))?;
    let meta = fs::metadata(path_ref).map_err(|e| anyhow::anyhow!("cannot inspect {path}: {e}"))?;
    let mode = meta.permissions().mode() & 0o777;
    let file_name = path_ref
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("{path} has no file name"))?
        .to_string_lossy();
    let tmp_path = parent.join(format!(".{file_name}.facegate.{}.tmp", std::process::id()));

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
        fs::rename(&tmp_path, path_ref).map_err(|e| {
            anyhow::anyhow!("cannot replace {path} with {}: {e}", tmp_path.display())
        })?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    write_result
}
