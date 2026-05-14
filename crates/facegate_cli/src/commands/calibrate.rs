use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::{bail, Context as _};
use facegate_core::config::Config;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::AuthScope;

use crate::commands::{broker, services};

const DEFAULT_MARGIN: f32 = 0.05;
const MIN_RECOMMENDED_THRESHOLD: f32 = 0.30;
const MAX_RECOMMENDED_THRESHOLD: f32 = 0.95;

#[derive(Debug, Clone, Copy)]
pub struct CalibrationStats {
    pub min: f32,
    pub median: f32,
    pub max: f32,
    pub average: f32,
    pub recommended: f32,
}

pub fn run(
    mut config: Config,
    config_path: PathBuf,
    username: &str,
    auth_scope: AuthScope,
    samples: u32,
    write: bool,
) -> anyhow::Result<()> {
    if samples == 0 {
        bail!("--samples must be greater than zero");
    }

    let enrolled = broker::list_templates(username)?
        .into_iter()
        .filter(|template| broker::summary_allows(template, auth_scope))
        .count();
    if enrolled == 0 {
        bail!(
            "user has no enrolled templates for {}",
            scope_label(auth_scope)
        );
    }

    println!("Facegate threshold calibration");
    println!("User      : {username}");
    println!("Scope     : {}", scope_label(auth_scope));
    println!("Samples   : {samples}");
    println!(
        "Threshold : {:.4}",
        config.recognition.policy_for(auth_scope).threshold
    );
    println!();
    println!("This captures live positive samples and compares them via the broker.");
    println!("No templates are overwritten.");
    println!();

    let mut pipeline = FacePipeline::new(&config)?;
    let mut scores = Vec::with_capacity(samples as usize);

    for index in 1..=samples {
        wait_for_enter(index, samples)?;
        let embedding = pipeline.capture_embedding(&config)?;
        let result = broker::match_embedding(username, auth_scope, embedding)?;
        let score = result
            .score
            .ok_or_else(|| anyhow::anyhow!("broker returned no score for sample {index}"))?;
        let verdict = if result.matched { "ACCEPT" } else { "REJECT" };
        println!("Sample {index:>2}/{samples}: {score:.4} ({verdict})");
        scores.push(score);
    }

    println!();
    let stats = calibration_stats(&scores)?;
    let current_threshold = config.recognition.policy_for(auth_scope).threshold;
    print_stats(&stats, current_threshold);

    if stats.recommended < current_threshold {
        println!(
            "Note: recommendation is below the current threshold because at least one positive sample scored low."
        );
    }

    if write {
        println!();
        if ask_yes_no(
            &format!(
                "Write {}.threshold = {:.4} to {}?",
                scope_config_label(auth_scope),
                stats.recommended,
                config_path.display()
            ),
            false,
        )? {
            backup_config(&config_path)?;
            match auth_scope {
                AuthScope::Sudo => config.recognition.sudo.threshold = Some(stats.recommended),
                AuthScope::Session => {
                    config.recognition.session.threshold = Some(stats.recommended)
                }
            }
            write_config(&config_path, &config)?;
            println!("Config updated.");
            services::print_refresh_summary(&services::refresh_after_config_change());
        } else {
            println!("Config unchanged.");
        }
    } else {
        println!();
        println!("Config unchanged. Re-run with --write to apply the recommendation.");
    }

    Ok(())
}

pub fn run_streaming(
    config: &Config,
    username: &str,
    auth_scope: AuthScope,
    samples: u32,
    tx: &Sender<String>,
) -> anyhow::Result<()> {
    if samples == 0 {
        bail!("samples must be greater than zero");
    }

    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }

    let enrolled = broker::list_templates(username)?
        .into_iter()
        .filter(|template| broker::summary_allows(template, auth_scope))
        .count();
    if enrolled == 0 {
        bail!(
            "user has no enrolled templates for {}",
            scope_label(auth_scope)
        );
    }

    let current_threshold = config.recognition.policy_for(auth_scope).threshold;
    out!("Facegate threshold calibration");
    out!("User      : {username}");
    out!("Scope     : {}", scope_label(auth_scope));
    out!("Samples   : {samples}");
    out!("Threshold : {current_threshold:.4}");
    out!("");
    out!("Capturing live positive samples. Keep your face visible and steady.");
    out!("No templates or config files will be changed from the TUI.");
    out!("");

    let mut pipeline = FacePipeline::new(config)?;
    let mut scores = Vec::with_capacity(samples as usize);

    for index in 1..=samples {
        out!("Capturing sample {index}/{samples}...");
        let embedding = pipeline.capture_embedding(config)?;
        let result = broker::match_embedding(username, auth_scope, embedding)?;
        let score = result
            .score
            .ok_or_else(|| anyhow::anyhow!("broker returned no score for sample {index}"))?;
        let verdict = if result.matched { "ACCEPT" } else { "REJECT" };
        out!("  sample {index:>2}/{samples}: {score:.4} ({verdict})");
        scores.push(score);
    }

    out!("");
    let stats = calibration_stats(&scores)?;
    push_stats(&stats, current_threshold, tx);
    if stats.recommended < current_threshold {
        out!(
            "Note: recommendation is below the current threshold because at least one positive sample scored low."
        );
    }
    out!("");
    out!(
        "To apply: sudo facegate calibrate {username} --for {} --samples {samples} --write",
        scope_label(auth_scope)
    );

    Ok(())
}

pub fn calibration_stats(scores: &[f32]) -> anyhow::Result<CalibrationStats> {
    if scores.is_empty() {
        bail!("no scores collected");
    }
    if scores.iter().any(|score| !score.is_finite()) {
        bail!("all scores must be finite");
    }

    let mut sorted = scores.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));

    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let median = if sorted.len().is_multiple_of(2) {
        let upper = sorted.len() / 2;
        (sorted[upper - 1] + sorted[upper]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    };
    let average = sorted.iter().sum::<f32>() / sorted.len() as f32;
    let recommended = recommend_threshold(min);

    Ok(CalibrationStats {
        min,
        median,
        max,
        average,
        recommended,
    })
}

fn recommend_threshold(min_positive_score: f32) -> f32 {
    (min_positive_score - DEFAULT_MARGIN)
        .clamp(MIN_RECOMMENDED_THRESHOLD, MAX_RECOMMENDED_THRESHOLD)
}

fn print_stats(stats: &CalibrationStats, current_threshold: f32) {
    println!("Score distribution");
    println!("  min         : {:.4}", stats.min);
    println!("  median      : {:.4}", stats.median);
    println!("  average     : {:.4}", stats.average);
    println!("  max         : {:.4}", stats.max);
    println!("  current     : {current_threshold:.4}");
    println!("  recommended : {:.4}", stats.recommended);
    println!();
    println!(
        "Recommendation: lowest observed positive score ({:.4}) minus {:.2} safety margin, clamped to [{:.2}, {:.2}].",
        stats.min, DEFAULT_MARGIN, MIN_RECOMMENDED_THRESHOLD, MAX_RECOMMENDED_THRESHOLD
    );
}

fn push_stats(stats: &CalibrationStats, current_threshold: f32, tx: &Sender<String>) {
    macro_rules! out {
        ($($arg:tt)*) => {{ let _ = tx.send(format!($($arg)*)); }};
    }
    out!("Score distribution");
    out!("  min         : {:.4}", stats.min);
    out!("  median      : {:.4}", stats.median);
    out!("  average     : {:.4}", stats.average);
    out!("  max         : {:.4}", stats.max);
    out!("  current     : {current_threshold:.4}");
    out!("  recommended : {:.4}", stats.recommended);
    out!("");
    out!(
        "Recommendation: lowest observed positive score ({:.4}) minus {:.2} safety margin, clamped to [{:.2}, {:.2}].",
        stats.min, DEFAULT_MARGIN, MIN_RECOMMENDED_THRESHOLD, MAX_RECOMMENDED_THRESHOLD
    );
}

fn wait_for_enter(index: u32, total: u32) -> anyhow::Result<()> {
    print!("Press Enter to capture calibration sample {index}/{total}...");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(())
}

fn ask_yes_no(prompt: &str, default_yes: bool) -> anyhow::Result<bool> {
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

fn backup_config(config_path: &Path) -> anyhow::Result<()> {
    if !config_path.exists() {
        return Ok(());
    }
    let backup = config_path.with_extension(format!(
        "{}.bak",
        config_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("toml")
    ));
    std::fs::copy(config_path, &backup).with_context(|| {
        format!(
            "cannot back up {} to {}",
            config_path.display(),
            backup.display()
        )
    })?;
    println!("Backed up config to {}", backup.display());
    Ok(())
}

fn write_config(config_path: &Path, config: &Config) -> anyhow::Result<()> {
    let parent = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", config_path.display()))?;
    std::fs::create_dir_all(parent)?;
    let toml = toml::to_string_pretty(config)?;
    std::fs::write(config_path, toml)?;
    Ok(())
}

fn scope_label(scope: AuthScope) -> &'static str {
    match scope {
        AuthScope::Sudo => "sudo",
        AuthScope::Session => "session",
    }
}

fn scope_config_label(scope: AuthScope) -> &'static str {
    match scope {
        AuthScope::Sudo => "recognition.sudo",
        AuthScope::Session => "recognition.session",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_are_explainable_from_observed_scores() {
        let stats = calibration_stats(&[0.72, 0.68, 0.80, 0.70]).unwrap();
        assert_eq!(stats.min, 0.68);
        assert_eq!(stats.max, 0.80);
        assert!((stats.median - 0.71).abs() < 0.000_001);
        assert!((stats.average - 0.725).abs() < f32::EPSILON);
        assert!((stats.recommended - 0.63).abs() < f32::EPSILON);
    }

    #[test]
    fn recommendation_is_clamped() {
        assert_eq!(recommend_threshold(0.20), MIN_RECOMMENDED_THRESHOLD);
        assert_eq!(recommend_threshold(1.20), MAX_RECOMMENDED_THRESHOLD);
    }
}
