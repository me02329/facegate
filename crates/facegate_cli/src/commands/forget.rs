use std::io::{self, Write};

use anyhow::{bail, Result};
use facegate_core::config::Config;

use crate::commands::broker;

pub fn run(config: &Config, username: &str, skip_confirmation: bool) -> Result<()> {
    let _ = config;
    require_root()?;

    let templates = broker::list_templates(username)?;
    if templates.is_empty() {
        println!("No templates enrolled for '{username}'.");
        return Ok(());
    }

    println!(
        "This will permanently remove {} template(s) for '{username}':",
        templates.len()
    );
    for template in &templates {
        println!(
            "  #{:<3} {:<20} scope={} created={}",
            template.id,
            template.label,
            broker::summary_scope_label(template),
            template.created_at
        );
    }

    if !skip_confirmation && !confirm("Proceed? This cannot be undone.", false)? {
        println!("Cancelled — no templates removed.");
        return Ok(());
    }

    let mut removed = 0u32;
    let mut failures = Vec::new();
    for template in &templates {
        match broker::remove_template(username, template.id) {
            Ok(()) => removed += 1,
            Err(e) => failures.push((template.id, e.to_string())),
        }
    }

    println!(
        "Removed {removed}/{} template(s) for '{username}'.",
        templates.len()
    );
    for (id, err) in &failures {
        eprintln!("  failed to remove template #{id}: {err}");
    }
    if !failures.is_empty() {
        bail!("{} template(s) failed to remove", failures.len());
    }
    Ok(())
}

fn confirm(prompt: &str, default_yes: bool) -> Result<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{prompt} {suffix} ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let trimmed = line.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Ok(default_yes);
    }
    match trimmed.as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => bail!("expected yes or no"),
    }
}

fn require_root() -> Result<()> {
    // SAFETY: getuid() has no preconditions.
    if unsafe { libc::getuid() } != 0 {
        bail!("this command requires root privileges (run with sudo)");
    }
    Ok(())
}
