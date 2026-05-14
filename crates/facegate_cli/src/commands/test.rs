use std::sync::mpsc::Sender;

use facegate_core::camera::Frame;
use facegate_core::config::Config;
use facegate_core::storage::AuthScope;
use facegate_ipc::{FrameFormat, FrameProbe, MatchResult};

use crate::commands::{broker, user_log};

const CROSS_CHECK_CAPTURE_RETRIES: u32 = 3;

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
    user_log::append_for_user(
        username,
        format!("test start scope={scope_label} enrolled={enrolled_count}"),
    );
    if enrolled_count == 0 {
        return Ok(());
    }

    let auth_scopes = scopes_to_test(scope, &templates);
    if auth_scopes.is_empty() {
        return Ok(());
    }

    out!("Opening camera...");
    let mut camera = broker::open_rgb_camera(config)?;
    let mut ir_camera = if broker::cross_check_active(config) {
        out!("Opening IR camera...");
        Some(broker::open_ir_camera(config)?)
    } else {
        None
    };

    out!(
        "Looking for face (timeout: {}ms)...",
        config.camera.timeout_ms
    );
    let result = if let Some(ir_camera) = ir_camera.as_mut() {
        let mut selected = None;
        for capture_attempt in 1..=CROSS_CHECK_CAPTURE_RETRIES {
            let (rgb_result, ir_result) = broker::capture_rgb_ir_pair(&mut camera, ir_camera);
            let rgb_frame = rgb_result?;
            let ir_frame = ir_result?;
            out!("Captured synchronized RGB+IR probe; broker will run cross-check.");
            user_log::append_for_user(
                username,
                format!("test captured rgb_ir_probe attempt={capture_attempt}"),
            );
            let rgb_probe = frame_probe(rgb_frame);
            let ir_probe = frame_probe(ir_frame);
            let result = match_frame_pair_for_scopes(username, &auth_scopes, rgb_probe, ir_probe)?;
            if result.matched
                || result.score.is_some()
                || !broker::match_reason_is_retryable_capture(result.reason)
                || capture_attempt == CROSS_CHECK_CAPTURE_RETRIES
            {
                out!("");
                selected = Some(result);
                break;
            }
            out!(
                "Retrying RGB+IR capture ({capture_attempt}/{CROSS_CHECK_CAPTURE_RETRIES}) after {}.",
                broker::match_reason_human(result.reason)
            );
            user_log::append_for_user(
                username,
                format!(
                    "test retry attempt={capture_attempt} reason={}",
                    broker::match_reason_label(result.reason)
                ),
            );
        }
        selected.ok_or_else(|| anyhow::anyhow!("No enrolled templates to compare against."))?
    } else {
        let rgb_frame = camera.capture_frame()?;
        out!("Captured single-camera probe; broker will run detection + matching.\n");
        user_log::append_for_user(username, "test captured single_camera_probe");
        let probe = frame_probe(rgb_frame);
        match_frame_for_scopes(username, &auth_scopes, probe)?
    };

    let threshold = config.recognition.threshold;
    match result.score {
        None => {
            out!("No match score returned.");
            out!(
                "Reason          : {}",
                broker::match_reason_human(result.reason)
            );
            user_log::append_for_user(
                username,
                format!(
                    "test reject reason={}",
                    broker::match_reason_label(result.reason)
                ),
            );
        }
        Some(score) => {
            let label = if result.matched { "ACCEPT" } else { "REJECT" };
            let marker = if result.matched { "✓" } else { "✗" };
            out!("Best similarity : {score:.4}");
            out!("Threshold       : {threshold}");
            if !result.matched {
                out!(
                    "Reason          : {}",
                    broker::match_reason_human(result.reason)
                );
            }
            out!("Result          : [{marker}] {label}");
            user_log::append_for_user(
                username,
                format!(
                    "test result={} reason={} score={score:.4} threshold={threshold}",
                    label.to_ascii_lowercase(),
                    broker::match_reason_label(result.reason)
                ),
            );
        }
    }
    Ok(())
}

fn match_frame_pair_for_scopes(
    username: &str,
    auth_scopes: &[AuthScope],
    rgb_probe: FrameProbe,
    ir_probe: FrameProbe,
) -> anyhow::Result<MatchResult> {
    let mut results = Vec::new();
    for auth_scope in auth_scopes {
        results.push(broker::match_frame_pair(
            username,
            *auth_scope,
            rgb_probe.clone(),
            ir_probe.clone(),
        )?);
    }
    results
        .into_iter()
        .reduce(best_result)
        .ok_or_else(|| anyhow::anyhow!("No enrolled templates to compare against."))
}

fn match_frame_for_scopes(
    username: &str,
    auth_scopes: &[AuthScope],
    probe: FrameProbe,
) -> anyhow::Result<MatchResult> {
    let mut results = Vec::new();
    for auth_scope in auth_scopes {
        results.push(broker::match_frame(username, *auth_scope, probe.clone())?);
    }
    results
        .into_iter()
        .reduce(best_result)
        .ok_or_else(|| anyhow::anyhow!("No enrolled templates to compare against."))
}

fn scopes_to_test(
    scope: TestScope,
    templates: &[facegate_ipc::EnrolledTemplateSummary],
) -> Vec<AuthScope> {
    match scope {
        TestScope::Auth(scope) => vec![scope],
        TestScope::All => [AuthScope::Session, AuthScope::Sudo]
            .into_iter()
            .filter(|scope| {
                templates
                    .iter()
                    .any(|template| broker::summary_allows(template, *scope))
            })
            .collect(),
    }
}

fn frame_probe(frame: Frame) -> FrameProbe {
    FrameProbe {
        format: FrameFormat::Rgb8,
        width: frame.width,
        height: frame.height,
        captured_at_ms: frame.captured_at_ms,
        bytes: frame.data,
    }
}

fn best_result(left: MatchResult, right: MatchResult) -> MatchResult {
    match (left.matched, right.matched) {
        (true, false) => return left,
        (false, true) => return right,
        _ => {}
    }
    match (left.score, right.score) {
        (Some(a), Some(b)) if b > a => right,
        (None, Some(_)) => right,
        _ => left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use facegate_ipc::{EnrolledTemplateSummary, MatchReason, TemplateScope};

    #[test]
    fn all_scope_uses_only_scopes_with_templates() {
        let templates = vec![EnrolledTemplateSummary {
            id: 0,
            label: "session".to_owned(),
            created_at: "now".to_owned(),
            scope: TemplateScope::Session,
        }];
        assert_eq!(
            scopes_to_test(TestScope::All, &templates),
            vec![AuthScope::Session]
        );
    }

    #[test]
    fn both_template_tests_both_auth_scopes() {
        let templates = vec![EnrolledTemplateSummary {
            id: 0,
            label: "both".to_owned(),
            created_at: "now".to_owned(),
            scope: TemplateScope::Both,
        }];
        assert_eq!(
            scopes_to_test(TestScope::All, &templates),
            vec![AuthScope::Session, AuthScope::Sudo]
        );
    }

    #[test]
    fn best_result_prefers_accept_over_higher_reject_score() {
        let accepted = MatchResult {
            matched: true,
            score: Some(0.65),
            template_id: Some(1),
            reason: MatchReason::Matched,
        };
        let rejected = MatchResult {
            matched: false,
            score: Some(0.80),
            template_id: None,
            reason: MatchReason::TemplateMismatch,
        };

        assert!(best_result(accepted.clone(), rejected.clone()).matched);
        assert!(best_result(rejected, accepted).matched);
    }
}
