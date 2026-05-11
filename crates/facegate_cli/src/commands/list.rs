use std::sync::mpsc::Sender;

use facegate_core::config::Config;
use facegate_core::storage::EnrolledTemplate;

use crate::commands::broker;

pub fn load_templates(config: &Config, username: &str) -> anyhow::Result<Vec<EnrolledTemplate>> {
    let _ = config;
    broker::list_templates(username).map(|templates| {
        templates
            .into_iter()
            .map(broker::summary_to_enrolled)
            .collect()
    })
}

pub fn run(config: &Config, username: &str) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let config = config.clone();
    let username = username.to_owned();
    let handle = std::thread::spawn(move || run_streaming(&config, Some(&username), &tx));

    for line in rx {
        println!("{line}");
    }

    handle
        .join()
        .map_err(|_| anyhow::anyhow!("thread panicked"))??;
    Ok(())
}

pub fn run_streaming(
    config: &Config,
    username: Option<&str>,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    let username = username.unwrap_or("");
    macro_rules! out {
        ($($arg:tt)*) => {{
            let _ = tx.send(format!($($arg)*));
        }};
    }

    let _ = config;
    let templates = broker::list_templates(username)?;

    if templates.is_empty() {
        out!("No enrolled templates for '{username}'.");
        return Ok(());
    }

    out!("Templates for '{username}':\n");
    out!("  {:<4}  {:<20}  {:<8}  Label", "ID", "Created", "Scope");
    out!("  {}", "─".repeat(57));
    for t in &templates {
        out!(
            "  {:<4}  {:<20}  {:<8}  {}",
            t.id,
            t.created_at,
            broker::summary_scope_label(t),
            t.label
        );
    }
    Ok(())
}
