use std::sync::mpsc::Sender;

use facegate_core::config::Config;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::AuthScope;

use crate::commands::broker;

#[derive(Debug, Clone, Copy)]
pub enum TestScope {
    /// Match against every enrolled template, regardless of scope.
    All,
    /// Match only against templates allowed for the given auth scope.
    Auth(AuthScope),
}

pub fn run(config: &Config, username: &str, scope: TestScope) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let config = config.clone();
    let username = username.to_owned();
    let handle = std::thread::spawn(move || run_streaming(&config, Some(&username), scope, &tx));

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
    scope: TestScope,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    let username = username.unwrap_or("");
    macro_rules! out {
        ($($arg:tt)*) => {{
            let _ = tx.send(format!($($arg)*));
        }};
    }

    let templates = broker::list_templates(username)?;
    let enrolled_count = match scope {
        TestScope::All => templates.len(),
        TestScope::Auth(s) => templates
            .iter()
            .filter(|template| broker::summary_allows(template, s))
            .count(),
    };
    let scope_label = match scope {
        TestScope::All => "any",
        TestScope::Auth(AuthScope::Sudo) => "sudo",
        TestScope::Auth(AuthScope::Session) => "session",
    };
    out!(
        "Found {} enrolled template(s) for '{username}' (scope: {scope_label}).",
        enrolled_count
    );
    if enrolled_count == 0 {
        return Ok(());
    }

    out!("Opening camera and loading models...");
    let mut pipeline = FacePipeline::new(config)?;

    out!(
        "Looking for face (timeout: {}ms)...",
        config.camera.timeout_ms
    );
    let embedding = pipeline.capture_embedding(config)?;
    out!("Face detected.\n");

    let result = match scope {
        TestScope::Auth(s) => broker::match_embedding(username, s, embedding)?,
        TestScope::All => {
            let mut results = Vec::new();
            if templates
                .iter()
                .any(|template| broker::summary_allows(template, AuthScope::Session))
            {
                if let Some(result) = broker::match_embedding_optional(
                    username,
                    AuthScope::Session,
                    embedding.clone(),
                )? {
                    results.push(result);
                }
            }
            if templates
                .iter()
                .any(|template| broker::summary_allows(template, AuthScope::Sudo))
            {
                if let Some(result) =
                    broker::match_embedding_optional(username, AuthScope::Sudo, embedding)?
                {
                    results.push(result);
                }
            }
            results
                .into_iter()
                .reduce(best_result)
                .ok_or_else(|| anyhow::anyhow!("No enrolled templates to compare against."))?
        }
    };

    let threshold = config.recognition.threshold;
    match result.score {
        None => out!("No enrolled templates to compare against."),
        Some(score) => {
            let label = if result.matched { "ACCEPT" } else { "REJECT" };
            let marker = if result.matched { "✓" } else { "✗" };
            out!("Best similarity : {score:.4}");
            out!("Threshold       : {threshold}");
            out!("Result          : [{marker}] {label}");
        }
    }
    Ok(())
}

fn best_result(
    left: facegate_ipc::MatchResult,
    right: facegate_ipc::MatchResult,
) -> facegate_ipc::MatchResult {
    match (left.score, right.score) {
        (Some(a), Some(b)) if b > a => right,
        (None, Some(_)) => right,
        _ => left,
    }
}
