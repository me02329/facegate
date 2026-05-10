use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use anyhow::bail;
use facegate_core::config::Config;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::{TemplateScope, TemplateStore};

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
    let store = TemplateStore::new(&config.storage.base_dir);

    for i in 1..=samples {
        out!("");
        if interactive {
            out!("Sample {i}/{samples} — position yourself in front of the camera, then press Enter...");
            wait_for_enter()?;
        }
        out!("Opening camera and loading models...");
        let mut pipeline = FacePipeline::new(config)?;
        out!("Capturing (timeout: {}ms)...", config.camera.timeout_ms);
        let embedding = pipeline.capture_embedding(config)?;
        let sample_label = if samples == 1 {
            label.to_owned()
        } else {
            format!("{label}-{i}")
        };
        let template =
            store.add_template(username, &sample_label, target.template_scope(), embedding)?;
        out!(
            "  ✓ template #{} saved (label: '{sample_label}')",
            template.id
        );
    }

    // The facegate-watch daemon runs as the user (not root). Chown the template
    // directory and file so the daemon can read them. Ownership is transferred
    // to the user; root writes on re-enrollment will be corrected by the next
    // chown call, so this is always safe to repeat.
    if matches!(target, EnrollmentTarget::Session | EnrollmentTarget::Both) {
        chown_user_data_dir(username, &config.storage.base_dir)?;
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

/// Transfers ownership of the user's template directory and `embeddings.json`
/// to the enrolled user so that `facegate-watch` (which runs as the user, not
/// root) can read the templates.  Permissions stay at 0700/0600 — only the
/// owner changes, so no other user gains access.
fn chown_user_data_dir(username: &str, base_dir: &std::path::Path) -> anyhow::Result<()> {
    let c_name = std::ffi::CString::new(username)
        .map_err(|_| anyhow::anyhow!("invalid username '{username}'"))?;
    // SAFETY: getpwnam is thread-safe with respect to our single-threaded use here.
    let uid = unsafe {
        let pw = libc::getpwnam(c_name.as_ptr());
        if pw.is_null() {
            bail!("cannot resolve UID for '{username}'");
        }
        (*pw).pw_uid
    };
    // -1 (all bits set) means "leave gid unchanged" in POSIX chown.
    let keep_gid = u32::MAX;

    let user_dir = base_dir.join(username);
    for path in [user_dir.clone(), user_dir.join("embeddings.json")] {
        if !path.exists() {
            continue;
        }
        let c_path = std::ffi::CString::new(path.to_str().unwrap_or(""))
            .map_err(|_| anyhow::anyhow!("non-UTF-8 path: {}", path.display()))?;
        // SAFETY: chown with a valid CString path and numeric uid/gid is safe.
        let ret = unsafe { libc::chown(c_path.as_ptr(), uid, keep_gid) };
        if ret != 0 {
            bail!(
                "chown {username} {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            );
        }
    }
    Ok(())
}

fn require_root() -> anyhow::Result<()> {
    if unsafe { libc::getuid() } != 0 {
        bail!("this command requires root privileges (run with sudo)");
    }
    Ok(())
}

fn require_system_user(username: &str) -> anyhow::Result<()> {
    let c_username = std::ffi::CString::new(username)
        .map_err(|_| anyhow::anyhow!("invalid username '{username}'"))?;
    // SAFETY: getpwnam reads a NUL-terminated string and returns a borrowed
    // pointer owned by libc. We only test for null before the next libc call.
    let exists = unsafe { !libc::getpwnam(c_username.as_ptr()).is_null() };
    if !exists {
        bail!("system user '{username}' does not exist");
    }
    Ok(())
}

fn require_sudo_user(username: &str) -> anyhow::Result<()> {
    let status = Command::new("sudo")
        .args(["-n", "-l", "-U", username])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| anyhow::anyhow!("cannot check sudo privileges for '{username}': {e}"))?;

    if !status.success() {
        bail!("system user '{username}' does not have sudo privileges");
    }
    Ok(())
}
