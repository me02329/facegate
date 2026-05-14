use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use crate::commands::services::RefreshState;

const SERVICE: &str = "facegate-watch";

/// Returns `(username, uid)` for the user who invoked sudo, or `None` if not
/// running under sudo or if the user is root itself.
fn real_user() -> Option<(String, u32)> {
    let user = std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root")?;
    let uid = uid_for(&user)?;
    Some((user, uid))
}

fn uid_for(username: &str) -> Option<u32> {
    let c_name = std::ffi::CString::new(username).ok()?;
    let mut buf = vec![0i8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwnam_r writes into `pwd`/`buf` (both owned, large enough)
    // and reads `c_name` (NUL-terminated). It is the thread-safe form of
    // getpwnam — the buffer must outlive the returned pwd, which it does
    // since we only read pw_uid before this function returns.
    let rc = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    Some(pwd.pw_uid)
}

/// Run a `systemctl --user` sub-command as the real (non-root) user.
///
/// Prefers `runuser` (util-linux) and falls back to `sudo -u`. Both drop
/// privileges to `user` while keeping the correct `XDG_RUNTIME_DIR` and
/// D-Bus socket path so that systemd's user manager is reachable.
fn systemctl_user(user: &str, uid: u32, args: &[&str]) -> bool {
    let xdg = format!("/run/user/{uid}");
    let dbus = format!("unix:path=/run/user/{uid}/bus");

    let try_with = |program: &str, prefix_args: &[&str]| -> Option<bool> {
        let status = Command::new(program)
            .args(prefix_args)
            .args(["systemctl", "--user"])
            .args(args)
            .env("XDG_RUNTIME_DIR", &xdg)
            .env("DBUS_SESSION_BUS_ADDRESS", &dbus)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        Some(status.success())
    };

    if let Some(ok) = try_with("runuser", &["-u", user, "--"]) {
        return ok;
    }
    // BusyBox/Alpine images often ship without runuser — sudo is then the
    // canonical drop-privileges helper.
    try_with(
        "sudo",
        &[
            "-u",
            user,
            "--preserve-env=XDG_RUNTIME_DIR,DBUS_SESSION_BUS_ADDRESS",
            "--",
        ],
    )
    .unwrap_or(false)
}

/// Returns `true` if `facegate-watch.service` is currently active (running).
pub fn is_active() -> bool {
    let Some((user, uid)) = real_user() else {
        return false;
    };
    systemctl_user(&user, uid, &["is-active", "--quiet", SERVICE])
}

pub fn restart_if_active() -> RefreshState {
    if !is_installed() {
        return RefreshState::NotRunning;
    }
    let Some((user, uid)) = real_user() else {
        return RefreshState::NotRunning;
    };
    if !systemctl_user(&user, uid, &["is-active", "--quiet", SERVICE]) {
        return RefreshState::NotRunning;
    }
    if systemctl_user(&user, uid, &["restart", SERVICE]) {
        RefreshState::Restarted
    } else {
        RefreshState::Failed
    }
}

/// Returns `true` if the service unit file is installed at either the
/// system-wide or user-override location.
pub fn is_installed() -> bool {
    std::path::Path::new("/usr/lib/systemd/user/facegate-watch.service").exists()
        || std::path::Path::new("/etc/systemd/user/facegate-watch.service").exists()
}

pub fn run_streaming(enable: bool, tx: &Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    if !is_installed() {
        out!("Service unit not found at /usr/lib/systemd/user/facegate-watch.service.");
        out!("");
        out!("Reinstall facegate from your distro's package, or run install-dev.sh");
        out!("from the source tree if you built from source.");
        return Ok(());
    }

    let Some((user, uid)) = real_user() else {
        out!("Cannot determine the real user (SUDO_USER not set).");
        out!("");
        out!("Run the toggle as your normal user:");
        if enable {
            out!("  systemctl --user enable --now {SERVICE}");
        } else {
            out!("  systemctl --user disable --now {SERVICE}");
        }
        return Ok(());
    };

    let args: &[&str] = if enable {
        &["enable", "--now", SERVICE]
    } else {
        &["disable", "--now", SERVICE]
    };

    let ok = systemctl_user(&user, uid, args);

    if enable {
        if ok {
            out!("Watch daemon enabled and started for '{user}'.");
            out!("");
            out!("facegate-watch is running and will start automatically at");
            out!("every login. When the screen locks, face recognition starts");
            out!("immediately — no need to press Enter first.");
        } else {
            out!("Could not start the service automatically.");
            out!("");
            out!("Enable it manually as '{user}':");
            out!("  systemctl --user enable --now {SERVICE}");
        }
    } else if ok {
        out!("Watch daemon disabled and stopped for '{user}'.");
    } else {
        out!("Could not stop the service automatically.");
        out!("");
        out!("Disable it manually as '{user}':");
        out!("  systemctl --user disable --now {SERVICE}");
    }

    Ok(())
}
