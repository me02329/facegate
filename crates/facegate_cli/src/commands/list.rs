use std::sync::mpsc::Sender;

use facegate_core::config::Config;
use facegate_core::storage::{EnrolledTemplate, TemplateStore};

pub fn load_templates(config: &Config, username: &str) -> anyhow::Result<Vec<EnrolledTemplate>> {
    let store = TemplateStore::new(&config.storage.base_dir);
    Ok(store.load(username)?.templates)
}

pub fn run(config: &Config, username: &str) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    run_streaming(config, Some(username), &tx)?;
    drop(tx);
    for line in rx {
        println!("{line}");
    }
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

    let store = TemplateStore::new(&config.storage.base_dir);
    let templates = store.load(username)?;

    if templates.templates.is_empty() {
        out!("No enrolled templates for '{username}'.");
        return Ok(());
    }

    out!("Templates for '{username}':\n");
    out!("  {:<4}  {:<20}  {:<8}  Label", "ID", "Created", "Scope");
    out!("  {}", "─".repeat(57));
    for t in &templates.templates {
        out!(
            "  {:<4}  {:<20}  {:<8}  {}",
            t.id,
            t.created_at,
            t.scope.label(),
            t.label
        );
    }
    Ok(())
}
