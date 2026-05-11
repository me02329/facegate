use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _};
use facegate_core::config::Config;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::AuthScope;

use crate::commands::broker;

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
    println!("Threshold : {:.4}", config.recognition.threshold);
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
    print_stats(&stats, config.recognition.threshold);

    if stats.recommended < config.recognition.threshold {
        println!(
            "Note: recommendation is below the current threshold because at least one positive sample scored low."
        );
    }

    if write {
        println!();
        if ask_yes_no(
            &format!(
                "Write recognition.threshold = {:.4} to {}?",
                stats.recommended,
                config_path.display()
            ),
            false,
        )? {
            backup_config(&config_path)?;
            config.recognition.threshold = stats.recommended;
            write_config(&config_path, &config)?;
            println!("Config updated.");
        } else {
            println!("Config unchanged.");
        }
    } else {
        println!();
        println!("Config unchanged. Re-run with --write to apply the recommendation.");
    }

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
    let median = if sorted.len() % 2 == 0 {
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
