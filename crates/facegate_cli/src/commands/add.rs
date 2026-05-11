use std::io::{self, BufRead, Write};
use std::sync::mpsc::Sender;

use anyhow::bail;
use facegate_core::config::Config;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::TemplateScope;

use crate::commands::broker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentTarget {
    Sudo,
    Session,
    Both,
}

impl EnrollmentTarget {
    pub fn requires_sudo_user(self) -> bool {
        matches!(self, EnrollmentTarget::Sudo | EnrollmentTarget::Both)
    }

    pub fn label(self) -> &'static str {
        match self {
            EnrollmentTarget::Sudo => "sudo",
            EnrollmentTarget::Session => "session",
            EnrollmentTarget::Both => "sudo+session",
        }
    }

    pub fn template_scope(self) -> TemplateScope {
        match self {
            EnrollmentTarget::Sudo => TemplateScope::Sudo,
            EnrollmentTarget::Session => TemplateScope::Session,
            EnrollmentTarget::Both => TemplateScope::Both,
        }
    }
}

pub fn run(
    config: &Config,
    username: &str,
    label: Option<&str>,
    target: EnrollmentTarget,
) -> anyhow::Result<()> {
    let samples = ask_sample_count()?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let config = config.clone();
    let username = username.to_owned();
    let label = label.map(|s| s.to_owned());

    let handle = std::thread::spawn(move || {
        run_streaming(
            &config,
            Some(&username),
            label.as_deref(),
            samples,
            true,
            target,
            &tx,
        )
    });

    for line in rx {
        println!("{line}");
    }

    handle
        .join()
        .map_err(|_| anyhow::anyhow!("thread panicked"))??;
    Ok(())
}

/// `interactive`: if true, wait for Enter before each capture (CLI).
///                if false, capture immediately (TUI).
pub fn run_streaming(
    config: &Config,
    username: Option<&str>,
    label: Option<&str>,
    samples: u32,
    interactive: bool,
    target: EnrollmentTarget,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    let username = username.unwrap_or("");
    let label = label.unwrap_or(username);
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    require_root()?;
    require_system_user(username)?;
    if target.requires_sudo_user() {
        require_sudo_user(username)?;
    }

    out!(
        "Enrolling {} face for '{username}' (label: '{label}', {samples} sample(s))",
        target.label()
    );

    // Open camera + load models *once* and reuse for every sample. Reopening
    // the V4L2 device + reloading the ONNX models for each sample (~1 s each
    // on CPU) was a noticeable UX cost on slower hardware.
    out!("Opening camera and loading models...");
    let mut pipeline = FacePipeline::new(config)?;

    // Enrollment is interactive — give the user time to settle in front of
    // the camera between samples instead of hard-failing on the auth-tuned
    // timeout (typically 5 s).
    let enroll_config = {
        let mut c = config.clone();
        c.camera.timeout_ms = c.camera.timeout_ms.max(15_000);
        c
    };

    for i in 1..=samples {
        out!("");
        if interactive {
            out!("Sample {i}/{samples} — position yourself in front of the camera, then press Enter...");
            wait_for_enter()?;
        }
        out!(
            "Capturing (timeout: {}ms)...",
            enroll_config.camera.timeout_ms
        );
        let embedding = pipeline.capture_embedding(&enroll_config)?;
        let sample_label = if samples == 1 {
            label.to_owned()
        } else {
            format!("{label}-{i}")
        };
        let template =
            broker::enroll_template(username, &sample_label, target.template_scope(), embedding)?;
        out!(
            "  ✓ template #{} saved (label: '{sample_label}')",
            template.id
        );
    }

    out!("");
    out!("Done — {samples} template(s) enrolled for '{username}'.");
    Ok(())
}

fn wait_for_enter() -> anyhow::Result<()> {
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(())
}

fn ask_sample_count() -> anyhow::Result<u32> {
    print!("How many samples do you want to capture? [1-10, default 3]: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(3);
    }
    match trimmed.parse::<u32>() {
        Ok(n) if (1..=10).contains(&n) => Ok(n),
        _ => bail!("invalid number of samples '{trimmed}': expected 1-10"),
    }
}

fn require_root() -> anyhow::Result<()> {
    if unsafe { libc::getuid() } != 0 {
        bail!("this command requires root privileges (run with sudo)");
    }
    Ok(())
}

fn require_system_user(username: &str) -> anyhow::Result<()> {
    if lookup_uid(username)?.is_none() {
        bail!("system user '{username}' does not exist");
    }
    Ok(())
}

/// Thread-safe replacement for `getpwnam`. Returns `Ok(None)` if the user
/// does not exist (as opposed to a hard error).
fn lookup_uid(username: &str) -> anyhow::Result<Option<u32>> {
    let c_name = std::ffi::CString::new(username)
        .map_err(|_| anyhow::anyhow!("invalid username '{username}'"))?;

    // 4 KiB is enough for any reasonable passwd entry.
    let mut buf = vec![0i8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    // SAFETY: getpwnam_r writes into `pwd` and `buf` (both owned and large
    // enough), reads the NUL-terminated `c_name`, and sets `result` to either
    // `&mut pwd` on success or NULL on "not found".
    let rc = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 {
        return Err(anyhow::anyhow!(
            "getpwnam_r('{username}') failed: {}",
            std::io::Error::from_raw_os_error(rc)
        ));
    }
    if result.is_null() {
        return Ok(None);
    }
    Ok(Some(pwd.pw_uid))
}

/// Returns true when the user belongs to a group that's traditionally granted
/// sudo privileges (`wheel` on RHEL-likes/Arch, `sudo` on Debian-likes,
/// `admin` on macOS-style configs).  We deliberately *don't* shell out to
/// `sudo -l`: it requires a working sudo binary, runs the sudoers parser
/// (slow), and was returning false negatives on systems that grant sudo via
/// `/etc/sudoers.d/*` user lines instead of group membership.
///
/// Group membership is a strong heuristic but not a guarantee. We therefore
/// only *warn* on a missing match instead of bailing — root has already
/// authorised the enrollment and may legitimately want to enroll an admin
/// who is whitelisted via `/etc/sudoers` directly.
fn require_sudo_user(username: &str) -> anyhow::Result<()> {
    if user_in_any_group(username, &["wheel", "sudo", "admin"]) {
        return Ok(());
    }
    eprintln!(
        "Warning: '{username}' is not in wheel/sudo/admin group; \
         only proceed if the user is explicitly granted sudo via /etc/sudoers."
    );
    Ok(())
}

fn user_in_any_group(username: &str, groups: &[&str]) -> bool {
    let c_user = match std::ffi::CString::new(username) {
        Ok(c) => c,
        Err(_) => return false,
    };
    for group in groups {
        let c_group = match std::ffi::CString::new(*group) {
            Ok(c) => c,
            Err(_) => continue,
        };
        // SAFETY: getgrnam_r-equivalent: we use getgrnam read-only here. We
        // accept the small thread-safety risk since we never call this in
        // parallel with itself; a stray sibling caller would only race on
        // the static buffer libc returns and at worst return false.
        let group_uid = unsafe {
            let g = libc::getgrnam(c_group.as_ptr());
            if g.is_null() {
                continue;
            }
            // Walk the gr_mem null-terminated array of usernames.
            let mut members = (*g).gr_mem;
            let mut found = false;
            while !members.is_null() && !(*members).is_null() {
                let m = std::ffi::CStr::from_ptr(*members);
                if m.to_bytes() == c_user.as_bytes() {
                    found = true;
                    break;
                }
                members = members.add(1);
            }
            found
        };
        if group_uid {
            return true;
        }
    }
    false
}
