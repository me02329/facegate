use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

use crate::commands::{pam_edit, watch_toggle};

const BROKER_SERVICE: &str = "facegate-brokerd.service";

pub fn run(dry_run: bool) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    run_streaming(dry_run, &tx)?;
    drop(tx);
    for line in rx {
        println!("{line}");
    }
    Ok(())
}

pub fn run_streaming(dry_run: bool, tx: &Sender<String>) -> anyhow::Result<()> {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    out!("Emergency disable");
    out!("");
    out!("This restores password-first PAM recovery and stops Facegate daemons.");
    out!("");
    out!("Commands you can run manually:");
    out!("  sudo facegate emergency-disable");
    out!("  sudo facegate session-auth");
    out!("  sudo systemctl disable --now {BROKER_SERVICE}");
    out!("  systemctl --user disable --now facegate-watch");
    out!("");

    let actions = pam_edit::plan_emergency_restore()?;
    if actions.is_empty() {
        out!("PAM rollback: no Facegate PAM changes found.");
    } else {
        out!("PAM rollback plan:");
        for action in &actions {
            out!("  - {}", action.describe());
        }
    }

    if dry_run {
        out!("");
        out!("Dry run only; no files or services were changed.");
        return Ok(());
    }

    if !actions.is_empty() {
        out!("");
        out!("Applying PAM rollback:");
        for action in &actions {
            pam_edit::apply_emergency_action(action)?;
            out!("  ok: {}", action.describe());
        }
    }

    out!("");
    out!("Stopping services:");
    disable_broker(tx);
    watch_toggle::run_streaming(false, tx)?;
    out!("");
    out!("Emergency disable complete. Keep this root shell open and test sudo/login.");

    Ok(())
}

fn disable_broker(tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    if !std::path::Path::new("/run/systemd/system").exists() {
        out!("  systemd not detected; broker service was not changed.");
        out!("  Manual stop if needed: sudo systemctl disable --now {BROKER_SERVICE}");
        return;
    }

    let status = Command::new("systemctl")
        .args(["disable", "--now", BROKER_SERVICE])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => out!("  broker disabled and stopped."),
        _ => {
            out!("  could not stop broker automatically.");
            out!("  Run manually: sudo systemctl disable --now {BROKER_SERVICE}");
        }
    }
}
