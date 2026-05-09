use std::sync::mpsc::Sender;

use face_rs_core::config::Config;
use face_rs_core::matching::best_similarity;
use face_rs_core::pipeline::FacePipeline;
use face_rs_core::storage::TemplateStore;

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
    let enrolled = store.embeddings_for(username)?;
    out!(
        "Found {} enrolled template(s) for '{username}'.",
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
