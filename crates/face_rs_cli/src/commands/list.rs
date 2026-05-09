use std::sync::mpsc::Sender;

use face_rs_core::config::Config;
use face_rs_core::storage::TemplateStore;

pub fn run(config: &Config, username: &str) -> anyhow::Result<()> {
    run_streaming(config, Some(username), &std::sync::mpsc::channel().0)
}

pub fn run_streaming(
    config: &Config,
    username: Option<&str>,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    let username = username.unwrap_or("");
    macro_rules! out { ($($arg:tt)*) => { let _ = tx.send(format!($($arg)*)); } }

    let store = TemplateStore::new(&config.storage.base_dir);
    let templates = store.load(username)?;

    if templates.templates.is_empty() {
        out!("No enrolled templates for '{username}'.");
        return Ok(());
    }

    out!("Templates for '{username}':\n");
    out!("  {:<4}  {:<20}  {}", "ID", "Created", "Label");
    out!("  {}", "─".repeat(46));
    for t in &templates.templates {
        out!("  {:<4}  {:<20}  {}", t.id, t.created_at, t.label);
    }
    Ok(())
}
