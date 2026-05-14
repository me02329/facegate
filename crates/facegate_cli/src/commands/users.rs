use std::sync::mpsc::Sender;

use anyhow::Result;
use facegate_ipc::{EnrolledUserSummary, OwnershipSummary, TemplateScope};

use crate::commands::broker;

pub fn run(json: bool) -> Result<()> {
    let users = broker::list_users()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&users)?);
        return Ok(());
    }
    print_table(&users);
    Ok(())
}

pub fn run_streaming(tx: &Sender<String>) -> Result<()> {
    let users = broker::list_users()?;
    push_table(&users, tx);
    Ok(())
}

fn print_table(users: &[EnrolledUserSummary]) {
    if users.is_empty() {
        println!("No enrolled users.");
        return;
    }
    println!(
        "{:<20} {:>9} {:<18} {:<20} {:<20} {:<10} {:<10}",
        "USER", "TEMPLATES", "SCOPES", "FIRST", "LAST", "DIR", "FILE"
    );
    for user in users {
        println!("{}", table_line(user));
    }
}

fn push_table(users: &[EnrolledUserSummary], tx: &Sender<String>) {
    if users.is_empty() {
        let _ = tx.send("No enrolled users.".to_owned());
        return;
    }
    let _ = tx.send(format!(
        "{:<20} {:>9} {:<18} {:<20} {:<20} {:<10} {:<10}",
        "USER", "TEMPLATES", "SCOPES", "FIRST", "LAST", "DIR", "FILE"
    ));
    for user in users {
        let _ = tx.send(table_line(user));
    }
}

fn table_line(user: &EnrolledUserSummary) -> String {
    format!(
        "{:<20} {:>9} {:<18} {:<20} {:<20} {:<10} {:<10}",
        user.username,
        user.template_count,
        scopes_label(&user.scopes),
        user.first_enrolled_at.as_deref().unwrap_or("-"),
        user.last_enrolled_at.as_deref().unwrap_or("-"),
        ownership_label(user.directory.as_ref()),
        ownership_label(user.embeddings_file.as_ref())
    )
}

fn scopes_label(scopes: &[TemplateScope]) -> String {
    if scopes.is_empty() {
        return "-".to_owned();
    }
    scopes
        .iter()
        .map(|scope| match scope {
            TemplateScope::Sudo => "sudo",
            TemplateScope::Session => "session",
            TemplateScope::Both => "both",
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn ownership_label(summary: Option<&OwnershipSummary>) -> String {
    match summary {
        Some(summary) if summary.ok => format!("{:o}:ok", summary.mode),
        Some(summary) => format!("{:o}:fix", summary.mode),
        None => "missing".to_owned(),
    }
}
