use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path};

/// Absolute path so PAM finds the module regardless of the distro's default
/// search dir (Arch: /usr/lib/security, Debian: /usr/lib/x86_64-linux-gnu/security,
/// Fedora: /usr/lib64/security). Falling back to a bare "pam_facegate.so" only
/// works on distros whose search path matches our install path.
pub const PAM_LINE: &str = "auth      sufficient    /usr/lib/security/pam_facegate.so";

/// Legacy bare-name form, recognised when reading existing PAM files so users
/// who installed an earlier version can still toggle it off.
const PAM_LINE_LEGACY: &str = "auth      sufficient    pam_facegate.so";

fn line_matches(line: &str) -> bool {
    let t = line.trim();
    t == PAM_LINE.trim() || t == PAM_LINE_LEGACY.trim()
}

pub fn file_has_line(path: &str) -> bool {
    fs::read_to_string(path)
        .map(|c| c.lines().any(line_matches))
        .unwrap_or(false)
}

pub fn service_exists(service: &str) -> bool {
    service_path(service).is_some()
}

pub fn service_has_line(service: &str) -> bool {
    let target = format!("/etc/pam.d/{service}");
    if Path::new(&target).exists() {
        return file_has_line(&target);
    }
    let vendor = format!("/usr/lib/pam.d/{service}");
    file_has_line(&vendor)
}

pub fn set_service_enabled(service: &str, enabled: bool) -> anyhow::Result<bool> {
    let target = format!("/etc/pam.d/{service}");
    let source = service_path(service)
        .ok_or_else(|| anyhow::anyhow!("PAM service '{service}' was not found"))?;
    let content =
        fs::read_to_string(&source).map_err(|e| anyhow::anyhow!("cannot read {source}: {e}"))?;
    set_enabled_with_content(&target, &source, &content, enabled)
}

pub fn set_enabled(path: &str, enabled: bool) -> anyhow::Result<bool> {
    let content =
        fs::read_to_string(path).map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
    set_enabled_with_content(path, path, &content, enabled)
}

fn set_enabled_with_content(
    target_path: &str,
    source_path: &str,
    content: &str,
    enabled: bool,
) -> anyhow::Result<bool> {
    let already_enabled = content.lines().any(line_matches);

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
        // Strips both the current absolute-path form and the legacy bare-name form.
        content
            .lines()
            .filter(|l| !line_matches(l))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    };

    backup_pam_file(source_path, target_path)?;
    write_pam_atomic(target_path, source_path, &new_content)?;
    Ok(true)
}

pub fn service_path(service: &str) -> Option<String> {
    let local = format!("/etc/pam.d/{service}");
    if Path::new(&local).exists() {
        return Some(local);
    }
    let vendor = format!("/usr/lib/pam.d/{service}");
    if Path::new(&vendor).exists() {
        return Some(vendor);
    }
    None
}

/// Maximum number of `.facegate.<ts>.bak` snapshots we keep next to a PAM file.
/// Older backups are pruned so a chatty user doesn't flood `/etc/pam.d/`.
const MAX_BACKUPS: usize = 3;

fn backup_pam_file(source_path: &str, target_path: &str) -> anyhow::Result<()> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = format!("{target_path}.facegate.{secs}.bak");
    fs::copy(source_path, &backup)
        .map_err(|e| anyhow::anyhow!("cannot create PAM backup {backup}: {e}"))?;
    prune_old_backups(target_path);
    Ok(())
}

fn prune_old_backups(target_path: &str) {
    let target = Path::new(target_path);
    let Some(dir) = target.parent() else { return };
    let Some(stem) = target.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let prefix = format!("{stem}.facegate.");

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut backups: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_str()?;
            if !name.starts_with(&prefix) || !name.ends_with(".bak") {
                return None;
            }
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((mtime, e.path()))
        })
        .collect();
    if backups.len() <= MAX_BACKUPS {
        return;
    }
    backups.sort_by_key(|b| std::cmp::Reverse(b.0)); // newest first
    for (_, path) in backups.into_iter().skip(MAX_BACKUPS) {
        let _ = fs::remove_file(path);
    }
}

fn write_pam_atomic(target_path: &str, source_path: &str, new_content: &str) -> anyhow::Result<()> {
    let source_ref = Path::new(source_path);
    let mode = fs::metadata(source_ref)
        .map_err(|e| anyhow::anyhow!("cannot inspect {source_path}: {e}"))?
        .permissions()
        .mode()
        & 0o777;

    write_file_atomic(target_path, new_content, mode)
}

fn write_file_atomic(path: &str, new_content: &str, mode: u32) -> anyhow::Result<()> {
    let path_ref = Path::new(path);
    let parent = path_ref
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{path} has no parent directory"))?;
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
