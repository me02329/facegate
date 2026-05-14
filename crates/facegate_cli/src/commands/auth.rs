use facegate_core::config::Config;
use facegate_core::error::{AuthExitCode, FaceRsError};
use facegate_core::storage::AuthScope;
use facegate_ipc::ErrorCode;

use crate::commands::broker::{
    capture_rgb_ir_pair, cross_check_active, frame_probe, open_ir_camera, open_rgb_camera,
};
use crate::commands::{broker, user_log};

const CROSS_CHECK_CAPTURE_RETRIES: u32 = 3;

/// Non-interactive authentication called by the PAM module.
/// Returns an exit code — caller must pass it to std::process::exit.
///
/// Since broker-side MatchFrame the client only captures frames; SCRFD + ArcFace and the
/// match decision live inside facegate-brokerd, which means a same-UID
/// attacker cannot bypass live capture by submitting a precomputed embedding.
pub fn run(config: &Config, username: &str, service: Option<&str>) -> AuthExitCode {
    tracing::info!("auth requested for '{username}'");
    user_log::append_for_user(
        username,
        format!("auth start service={}", service.unwrap_or("unknown")),
    );
    let auth_scope = auth_scope_for_service(service);
    let policy = config.recognition.policy_for(auth_scope);
    let cross_check = cross_check_active(config);

    let mut camera = match open_rgb_camera(config) {
        Ok(cam) => cam,
        Err(FaceRsError::Camera(msg)) => {
            eprintln!("Facegate: camera error: {msg}");
            user_log::append_for_user(
                username,
                format!("auth camera_error device=rgb error={msg}"),
            );
            return if config.security.deny_on_camera_error {
                AuthExitCode::CameraError
            } else {
                fallback_or_deny(config)
            };
        }
        Err(e) => {
            tracing::error!("camera open error: {e}");
            return AuthExitCode::InternalError;
        }
    };
    let mut ir_camera = if cross_check {
        match open_ir_camera(config) {
            Ok(cam) => Some(cam),
            Err(FaceRsError::Camera(msg)) => {
                eprintln!("Facegate: IR camera error: {msg}");
                user_log::append_for_user(
                    username,
                    format!("auth camera_error device=ir error={msg}"),
                );
                return if config.security.deny_on_camera_error {
                    AuthExitCode::CameraError
                } else {
                    fallback_or_deny(config)
                };
            }
            Err(e) => {
                tracing::error!("IR camera open error: {e}");
                return AuthExitCode::InternalError;
            }
        }
    } else {
        None
    };

    let mut matches = 0_u32;
    let mut saw_timeout = false;
    for attempt in 1..=policy.max_attempts {
        let result = if let Some(ir_camera) = ir_camera.as_mut() {
            let mut selected = None;
            for capture_attempt in 1..=CROSS_CHECK_CAPTURE_RETRIES {
                let (rgb_result, ir_result) = capture_rgb_ir_pair(&mut camera, ir_camera);
                let rgb_frame =
                    match auth_outcome_from_capture(rgb_result, username, attempt, "rgb") {
                        CaptureOutcome::Frame(frame) => frame,
                        CaptureOutcome::Timeout => {
                            saw_timeout = true;
                            continue;
                        }
                        CaptureOutcome::CameraError => return camera_error_or_fallback(config),
                        CaptureOutcome::InternalError => return AuthExitCode::InternalError,
                    };
                let ir_frame = match auth_outcome_from_capture(ir_result, username, attempt, "ir") {
                    CaptureOutcome::Frame(frame) => frame,
                    CaptureOutcome::Timeout => {
                        saw_timeout = true;
                        continue;
                    }
                    CaptureOutcome::CameraError => return camera_error_or_fallback(config),
                    CaptureOutcome::InternalError => return AuthExitCode::InternalError,
                };
                let result = broker::match_frame_pair_for_auth(
                    username,
                    auth_scope,
                    frame_probe(rgb_frame),
                    frame_probe(ir_frame),
                );
                match &result {
                    Ok(result)
                        if !result.matched
                            && result.score.is_none()
                            && broker::match_reason_is_retryable_capture(result.reason)
                            && capture_attempt < CROSS_CHECK_CAPTURE_RETRIES =>
                    {
                        user_log::append_for_user(
                            username,
                            format!(
                                "auth retry attempt={attempt} capture_attempt={capture_attempt} reason={}",
                                broker::match_reason_label(result.reason)
                            ),
                        );
                        continue;
                    }
                    _ => {
                        selected = Some(result);
                        break;
                    }
                }
            }
            match selected {
                Some(result) => result,
                None => continue,
            }
        } else {
            let frame =
                match auth_outcome_from_capture(camera.capture_frame(), username, attempt, "rgb") {
                    CaptureOutcome::Frame(frame) => frame,
                    CaptureOutcome::Timeout => {
                        saw_timeout = true;
                        continue;
                    }
                    CaptureOutcome::CameraError => return camera_error_or_fallback(config),
                    CaptureOutcome::InternalError => return AuthExitCode::InternalError,
                };
            broker::match_frame_for_auth(username, auth_scope, frame_probe(frame))
        };

        match result {
            Ok(result) if result.matched => {
                matches += 1;
                tracing::debug!(
                    attempt,
                    matches,
                    required = policy.required_matches,
                    score = result.score,
                    "face match accepted for attempt"
                );
                if matches >= policy.required_matches {
                    tracing::info!("auth succeeded for '{username}'");
                    eprintln!("[ facegate ] \u{2714} Face recognized: {username}");
                    user_log::append_for_user(
                        username,
                        format!(
                            "auth accept reason={} score={:?}",
                            broker::match_reason_label(result.reason),
                            result.score
                        ),
                    );
                    return AuthExitCode::Recognized;
                }
            }
            Ok(result) => {
                user_log::append_for_user(
                    username,
                    format!(
                        "auth reject attempt={attempt} reason={} score={:?}",
                        broker::match_reason_label(result.reason),
                        result.score
                    ),
                );
                if config.logging.log_failed_attempts {
                    tracing::warn!(
                        attempt,
                        score = result.score,
                        "auth failed for '{username}': face not recognised"
                    );
                }
            }
            Err(e) => {
                return handle_broker_error(config, username, e);
            }
        }
    }

    if saw_timeout {
        user_log::append_for_user(username, "auth final=timeout");
        return timeout_or_deny(config);
    }
    user_log::append_for_user(username, "auth final=not_recognized");
    fallback_or_deny(config)
}

enum CaptureOutcome {
    Frame(facegate_core::camera::Frame),
    Timeout,
    CameraError,
    InternalError,
}

fn auth_outcome_from_capture(
    result: std::result::Result<facegate_core::camera::Frame, FaceRsError>,
    username: &str,
    attempt: u32,
    device: &str,
) -> CaptureOutcome {
    match result {
        Ok(frame) => CaptureOutcome::Frame(frame),
        Err(FaceRsError::Timeout) => {
            tracing::warn!(
                attempt,
                device,
                "face auth capture timed out for '{username}'"
            );
            user_log::append_for_user(
                username,
                format!("auth timeout attempt={attempt} device={device}"),
            );
            CaptureOutcome::Timeout
        }
        Err(FaceRsError::Camera(msg)) => {
            eprintln!("Facegate: {device} camera error during capture: {msg}");
            user_log::append_for_user(
                username,
                format!("auth capture_error attempt={attempt} device={device} error={msg}"),
            );
            CaptureOutcome::CameraError
        }
        Err(e) => {
            tracing::error!("{device} capture error: {e}");
            CaptureOutcome::InternalError
        }
    }
}

fn camera_error_or_fallback(config: &Config) -> AuthExitCode {
    if config.security.deny_on_camera_error {
        AuthExitCode::CameraError
    } else {
        fallback_or_deny(config)
    }
}

fn auth_scope_for_service(service: Option<&str>) -> AuthScope {
    match service {
        Some("sudo") | Some("sudo-i") => AuthScope::Sudo,
        _ => AuthScope::Session,
    }
}

fn fallback_or_deny(config: &Config) -> AuthExitCode {
    if config.security.allow_password_fallback {
        AuthExitCode::NotRecognized
    } else {
        AuthExitCode::Denied
    }
}

fn timeout_or_deny(config: &Config) -> AuthExitCode {
    if config.security.allow_password_fallback {
        AuthExitCode::Timeout
    } else {
        AuthExitCode::Denied
    }
}

fn handle_broker_error(
    config: &Config,
    username: &str,
    error: broker::BrokerAuthError,
) -> AuthExitCode {
    match error {
        broker::BrokerAuthError::Broker(broker_error)
            if matches!(
                broker_error.code,
                ErrorCode::NotEnrolled | ErrorCode::RateLimited | ErrorCode::LockedOut
            ) =>
        {
            tracing::warn!(
                username,
                code = ?broker_error.code,
                "face auth unavailable for user"
            );
            user_log::append_for_user(
                username,
                format!("auth broker_unavailable code={:?}", broker_error.code),
            );
            fallback_or_deny(config)
        }
        other => {
            tracing::error!("broker match error: {other}");
            user_log::append_for_user(username, format!("auth broker_error error={other}"));
            AuthExitCode::InternalError
        }
    }
}
