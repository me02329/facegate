use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use facegate_core::config::Config;
use facegate_ipc::{Request, RequestEnvelope, Response, DEFAULT_SOCKET_PATH};

use crate::commands::services;

const BROKER_SERVICE: &str = "facegate-brokerd.service";
const FACEGATE_USER: &str = "facegate";
const FACEGATE_GROUP: &str = "facegate";

pub fn status(config: &Config) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    status_streaming(config, &tx)?;
    drop(tx);
    for line in rx {
        println!("{line}");
    }
    Ok(())
}

pub fn health(config: &Config) -> anyhow::Result<()> {
    match health_info() {
        Ok((protocol, version)) => {
            println!("Broker health: ok");
            println!("  socket  : {DEFAULT_SOCKET_PATH}");
            println!("  protocol: {protocol}");
            println!("  version : {version}");
            println!("  detector: {}", model_status(&config.models.detector));
            println!("  embedder: {}", model_status(&config.models.embedder));
            Ok(())
        }
        Err(e) => anyhow::bail!("broker health check failed: {e}"),
    }
}

pub fn health_streaming(config: &Config, tx: &Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    let (protocol, version) = health_info()?;
    out!("Broker health: ok");
    out!("  socket  : {DEFAULT_SOCKET_PATH}");
    out!("  protocol: {protocol}");
    out!("  version : {version}");
    out!("  detector: {}", model_status(&config.models.detector));
    out!("  embedder: {}", model_status(&config.models.embedder));
    Ok(())
}

pub fn restart() -> anyhow::Result<()> {
    if !systemd_available() {
        anyhow::bail!("systemd is not running; start /usr/bin/facegate-brokerd manually");
    }
    let ok = Command::new("systemctl")
        .args(["restart", BROKER_SERVICE])
        .stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("Broker restarted.");
        Ok(())
    } else {
        anyhow::bail!("systemctl restart {BROKER_SERVICE} failed")
    }
}

pub fn restart_streaming(tx: &Sender<String>) -> anyhow::Result<()> {
    if !systemd_available() {
        anyhow::bail!("systemd is not running; start /usr/bin/facegate-brokerd manually");
    }
    let ok = Command::new("systemctl")
        .args(["restart", BROKER_SERVICE])
        .stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        let _ = tx.send("Broker restarted.".to_owned());
        Ok(())
    } else {
        anyhow::bail!("systemctl restart {BROKER_SERVICE} failed")
    }
}

pub fn logs(lines: usize) -> anyhow::Result<()> {
    let status = Command::new("journalctl")
        .args(["-u", BROKER_SERVICE, "--no-pager", "-n"])
        .arg(lines.to_string())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("journalctl failed for {BROKER_SERVICE}")
    }
}

pub fn logs_streaming(lines: usize, tx: &Sender<String>) -> anyhow::Result<()> {
    let output = Command::new("journalctl")
        .args(["-u", BROKER_SERVICE, "--no-pager", "-n"])
        .arg(lines.to_string())
        .output()?;
    if !output.status.success() {
        anyhow::bail!("journalctl failed for {BROKER_SERVICE}");
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let _ = tx.send(line.to_owned());
    }
    Ok(())
}

pub fn repair_permissions(config: &Config) -> anyhow::Result<()> {
    let (uid, gid) = facegate_ids()?;
    let users_dir = &config.storage.base_dir;
    let audit_log = audit_log_path(config);

    ensure_dir(users_dir, uid, gid, 0o700)?;
    repair_users_tree(users_dir, uid, gid)?;
    ensure_audit_log(&audit_log, uid, gid)?;

    println!("Broker permissions repaired.");
    println!(
        "  users: {} owner=facegate:facegate mode=0700",
        users_dir.display()
    );
    println!(
        "  audit: {} owner=facegate:facegate mode=0600",
        audit_log.display()
    );
    Ok(())
}

pub fn repair_permissions_streaming(config: &Config, tx: &Sender<String>) -> anyhow::Result<()> {
    let (uid, gid) = facegate_ids()?;
    let users_dir = &config.storage.base_dir;
    let audit_log = audit_log_path(config);

    ensure_dir(users_dir, uid, gid, 0o700)?;
    repair_users_tree(users_dir, uid, gid)?;
    ensure_audit_log(&audit_log, uid, gid)?;

    let _ = tx.send("Broker permissions repaired.".to_owned());
    let _ = tx.send(format!(
        "  users: {} owner=facegate:facegate mode=0700",
        users_dir.display()
    ));
    let _ = tx.send(format!(
        "  audit: {} owner=facegate:facegate mode=0600",
        audit_log.display()
    ));
    Ok(())
}

pub fn status_streaming(config: &Config, tx: &Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    out!("Broker");
    out!(
        "  systemd active : {}",
        yes_no(services::is_broker_active())
    );
    out!(
        "  systemd enabled: {}",
        yes_no(services::is_broker_enabled())
    );
    match health_info() {
        Ok((protocol, version)) => {
            out!("  socket         : ok ({DEFAULT_SOCKET_PATH})");
            out!("  protocol       : {protocol}");
            out!("  version        : {version}");
        }
        Err(e) => out!("  socket         : unavailable ({e})"),
    }
    out!(
        "  peers          : not exposed by IPC v{}",
        facegate_ipc::PROTOCOL_VERSION
    );

    let audit_log = audit_log_path(config);
    match fs::metadata(&audit_log) {
        Ok(meta) => out!(
            "  audit log      : {} bytes ({})",
            meta.len(),
            audit_log.display()
        ),
        Err(e) => out!(
            "  audit log      : unavailable ({}: {e})",
            audit_log.display()
        ),
    }
    describe_path("users dir", &config.storage.base_dir, tx);
    describe_path("socket", Path::new(DEFAULT_SOCKET_PATH), tx);
    Ok(())
}

fn health_info() -> anyhow::Result<(u16, String)> {
    let response =
        facegate_ipc::send_request(DEFAULT_SOCKET_PATH, RequestEnvelope::new(Request::Health))?;
    match response.response {
        Response::Health { info } => Ok((info.protocol_version, info.broker_version)),
        Response::Error(error) => anyhow::bail!("{:?}: {}", error.code, error.message),
        other => anyhow::bail!("unexpected response: {other:?}"),
    }
}

fn describe_path(label: &str, path: &Path, tx: &Sender<String>) {
    let send = |line: String| {
        let _ = tx.send(line);
    };
    match fs::symlink_metadata(path) {
        Ok(meta) => send(format!(
            "  {label:<14}: {} mode={:04o} uid={} gid={}",
            path.display(),
            meta.permissions().mode() & 0o7777,
            meta.uid(),
            meta.gid()
        )),
        Err(e) => send(format!(
            "  {label:<14}: unavailable ({}: {e})",
            path.display()
        )),
    }
}

fn audit_log_path(config: &Config) -> PathBuf {
    config
        .storage
        .base_dir
        .parent()
        .map(|path| path.join("audit.log"))
        .unwrap_or_else(|| PathBuf::from("/var/lib/facegate/audit.log"))
}

fn systemd_available() -> bool {
    Path::new("/run/systemd/system").exists()
}

fn facegate_ids() -> anyhow::Result<(u32, u32)> {
    let uid = lookup_user(FACEGATE_USER)
        .ok_or_else(|| anyhow::anyhow!("system user '{FACEGATE_USER}' does not exist"))?;
    let gid = lookup_group(FACEGATE_GROUP)
        .ok_or_else(|| anyhow::anyhow!("system group '{FACEGATE_GROUP}' does not exist"))?;
    Ok((uid, gid))
}

fn lookup_user(name: &str) -> Option<u32> {
    let c_name = std::ffi::CString::new(name).ok()?;
    let mut buf = vec![0i8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc == 0 && !result.is_null() {
        Some(pwd.pw_uid)
    } else {
        None
    }
}

fn lookup_group(name: &str) -> Option<u32> {
    let c_name = std::ffi::CString::new(name).ok()?;
    let mut buf = vec![0i8; 4096];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();
    let rc = unsafe {
        libc::getgrnam_r(
            c_name.as_ptr(),
            &mut grp,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc == 0 && !result.is_null() {
        Some(grp.gr_gid)
    } else {
        None
    }
}

fn ensure_dir(path: &Path, uid: u32, gid: u32, mode: u32) -> anyhow::Result<()> {
    fs::create_dir_all(path)?;
    set_owner_mode(path, uid, gid, mode)
}

fn repair_users_tree(path: &Path, uid: u32, gid: u32) -> anyhow::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        anyhow::bail!("refusing to repair symlink {}", path.display());
    }
    if meta.is_dir() {
        set_owner_mode(path, uid, gid, 0o700)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            repair_users_tree(&entry.path(), uid, gid)?;
        }
    } else if meta.is_file() && path.file_name().and_then(|n| n.to_str()) == Some("embeddings.json")
    {
        set_owner_mode(path, uid, gid, 0o600)?;
    }
    Ok(())
}

fn ensure_audit_log(path: &Path, uid: u32, gid: u32) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        anyhow::bail!("refusing to repair symlink {}", path.display());
    }
    if !path.exists() {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
    }
    set_owner_mode(path, uid, gid, 0o600)
}

fn set_owner_mode(path: &Path, uid: u32, gid: u32, mode: u32) -> anyhow::Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        anyhow::bail!("refusing to chmod/chown symlink {}", path.display());
    }
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let rc = unsafe { libc::lchown(c_path.as_ptr(), uid, gid) };
    if rc != 0 {
        return Err(Into::into(std::io::Error::last_os_error()));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn model_status(path: &Path) -> String {
    if path.is_file() {
        format!("present ({})", path.display())
    } else {
        format!("missing ({})", path.display())
    }
}
