use std::sync::mpsc::Sender;

use facegate_core::config::Config;
use facegate_core::matching::best_similarity;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::{AuthScope, TemplateStore};

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

    let store = TemplateStore::new(&config.storage.base_dir);
    let enrolled = match scope {
        TestScope::All => store.embeddings_for(username)?,
        TestScope::Auth(s) => store.embeddings_for_scope(username, s)?,
    };
    let scope_label = match scope {
        TestScope::All => "any",
        TestScope::Auth(AuthScope::Sudo) => "sudo",
        TestScope::Auth(AuthScope::Session) => "session",
    };
    out!(
        "Found {} enrolled template(s) for '{username}' (scope: {scope_label}).",
        enrolled.len()
    );

    out!("Opening camera and loading models...");
    let mut pipeline = FacePipeline::new(config)?;

    out!(
        "Looking for face (timeout: {}ms)...",
        config.camera.timeout_ms
    );
    let embedding = pipeline.capture_embedding(config)?;
    out!("Face detected.\n");

    let threshold = config.recognition.threshold;
    match best_similarity(&embedding, &enrolled) {
        None => out!("No enrolled templates to compare against."),
        Some(score) => {
            let result = if score >= threshold {
                "ACCEPT"
            } else {
                "REJECT"
            };
            let result_color = if score >= threshold { "✓" } else { "✗" };
            out!("Best similarity : {score:.4}");
            out!("Threshold       : {threshold}");
            out!("Result          : [{result_color}] {result}");
        }
    }
    Ok(())
}
