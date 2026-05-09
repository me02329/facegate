use std::sync::mpsc::Sender;

use anyhow::bail;
use facegate_core::config::Config;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::TemplateStore;

pub fn run(config: &Config, username: &str, label: Option<&str>) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    run_streaming(config, Some(username), label, &tx)?;
    drop(tx);
    for line in rx {
        println!("{line}");
    }
    Ok(())
}

pub fn run_streaming(
    config: &Config,
    username: Option<&str>,
    label: Option<&str>,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    let username = username.unwrap_or("");
    let label = label.unwrap_or(username);
    macro_rules! out {
        ($($arg:tt)*) => {{
            let _ = tx.send(format!($($arg)*));
        }};
    }

    require_root()?;

    out!("Enrolling face for '{username}' (label: '{label}')");
    out!("Opening camera and loading models...");
    let mut pipeline = FacePipeline::new(config)?;

    out!(
        "Looking for face (timeout: {}ms)...",
        config.camera.timeout_ms
    );
    let embedding = pipeline.capture_embedding(config)?;

    out!("Face detected. Saving template...");
    let store = TemplateStore::new(&config.storage.base_dir);
    let template = store.add_template(username, label, embedding)?;

    out!("Done — template #{} saved for '{username}'.", template.id);
    Ok(())
}

fn require_root() -> anyhow::Result<()> {
    if unsafe { libc::getuid() } != 0 {
        bail!("this command requires root privileges (run with sudo)");
    }
    Ok(())
}
