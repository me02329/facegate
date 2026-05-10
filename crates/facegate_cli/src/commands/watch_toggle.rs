use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

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
    // SAFETY: getpwnam is safe to call with a valid NUL-terminated string.
    let uid = unsafe {
        let pw = libc::getpwnam(c_name.as_ptr());
        if pw.is_null() {
            return None;
        }
        (*pw).pw_uid
    };
    Some(uid)
}

/// Run a `systemctl --user` sub-command as the real (non-root) user.
///
/// Uses `runuser` (util-linux) to drop from root to the target user while
/// keeping the correct `XDG_RUNTIME_DIR` and D-Bus socket path so that
/// systemd's user manager is reachable.
fn systemctl_user(user: &str, uid: u32, args: &[&str]) -> bool {
    Command::new("runuser")
        .args(["-u", user, "--", "systemctl", "--user"])
        .args(args)
        .env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path=/run/user/{uid}/bus"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Returns `true` if `facegate-watch.service` is currently active (running).
pub fn is_active() -> bool {
    let Some((user, uid)) = real_user() else {
        return false;
    };
    systemctl_user(&user, uid, &["is-active", "--quiet", SERVICE])
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
        out!("Service unit not found.");
        out!("");
        out!("Install facegate to get the systemd unit file, then re-run.");
        out!("  sudo bash install-dev.sh");
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
