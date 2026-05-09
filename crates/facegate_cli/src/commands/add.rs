use std::io::{self, BufRead, Write};
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::bail;
use facegate_core::config::Config;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::TemplateStore;

pub fn run(config: &Config, username: &str, label: Option<&str>) -> anyhow::Result<()> {
    let samples = ask_sample_count()?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let config = config.clone();
    let username = username.to_owned();
    let label = label.map(|s| s.to_owned());

    let handle = std::thread::spawn(move || {
        run_streaming(&config, Some(&username), label.as_deref(), samples, &tx)
    });

    for line in rx {
        println!("{line}");
    }

    handle.join().map_err(|_| anyhow::anyhow!("thread panicked"))??;
    Ok(())
}

pub fn run_streaming(
    config: &Config,
    username: Option<&str>,
    label: Option<&str>,
    samples: u32,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    let username = username.unwrap_or("");
    let label = label.unwrap_or(username);
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    require_root()?;

    out!("Enrolling face for '{username}' (label: '{label}', {samples} sample(s))");
    out!("Opening camera and loading models...");
    let mut pipeline = FacePipeline::new(config)?;

    let store = TemplateStore::new(&config.storage.base_dir);

    for i in 1..=samples {
        out!("");
        out!("Sample {i}/{samples} — look at the camera, capturing in 3 seconds...");
        std::thread::sleep(Duration::from_secs(3));
        out!(
            "Capturing (timeout: {}ms)...",
            config.camera.timeout_ms
        );
        let embedding = pipeline.capture_embedding(config)?;
        let sample_label = if samples == 1 {
            label.to_owned()
        } else {
            format!("{label}-{i}")
        };
        let template = store.add_template(username, &sample_label, embedding)?;
        out!("  ✓ template #{} saved (label: '{sample_label}')", template.id);
    }

    out!("");
    out!("Done — {samples} template(s) enrolled for '{username}'.");
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
        Ok(n) if n >= 1 && n <= 10 => Ok(n),
        _ => bail!("invalid number of samples '{trimmed}': expected 1-10"),
    }
}

fn require_root() -> anyhow::Result<()> {
    if unsafe { libc::getuid() } != 0 {
        bail!("this command requires root privileges (run with sudo)");
    }
    Ok(())
}
