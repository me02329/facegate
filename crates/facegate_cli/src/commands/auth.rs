use facegate_core::config::Config;
use facegate_core::error::{AuthExitCode, FaceRsError};
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::AuthScope;
use facegate_ipc::ErrorCode;

use crate::commands::broker;

/// Non-interactive authentication called by the PAM module.
/// Returns an exit code — caller must pass it to std::process::exit.
pub fn run(config: &Config, username: &str, service: Option<&str>) -> AuthExitCode {
    tracing::info!("auth requested for '{username}'");
    let auth_scope = auth_scope_for_service(service);

    // Open camera + load models.
    let mut pipeline = match FacePipeline::new(config) {
        Ok(p) => p,
        Err(FaceRsError::Camera(msg)) => {
            // Print to stderr so the error appears in journalctl / PAM logs.
            // tracing is not initialised in auth mode, so this is the only trace.
            eprintln!("Facegate: camera error: {msg}");
            return if config.security.deny_on_camera_error {
                AuthExitCode::CameraError
            } else {
                // deny_on_camera_error = false → let PAM fall through to password
                fallback_or_deny(config)
            };
        }
        Err(FaceRsError::Detection(msg)) | Err(FaceRsError::Embedding(msg)) => {
            eprintln!("Facegate: model error: {msg}");
            return AuthExitCode::ConfigError;
        }
        Err(e) => {
            tracing::error!("pipeline init error: {e}");
            return AuthExitCode::InternalError;
        }
    };

    let mut matches = 0_u32;
    let mut saw_timeout = false;
    for attempt in 1..=config.recognition.max_attempts {
        let embedding = match pipeline.capture_embedding(config) {
            Ok(e) => e,
            Err(FaceRsError::Timeout) => {
                saw_timeout = true;
                tracing::warn!(attempt, "face auth timed out for '{username}'");
                continue;
            }
            Err(FaceRsError::Camera(msg)) => {
                eprintln!("Facegate: camera error during capture: {msg}");
                return if config.security.deny_on_camera_error {
                    AuthExitCode::CameraError
                } else {
                    fallback_or_deny(config)
                };
            }
            Err(FaceRsError::Detection(msg)) | Err(FaceRsError::Embedding(msg)) => {
                tracing::error!("model inference error: {msg}");
                return AuthExitCode::InternalError;
            }
            Err(e) => {
                tracing::error!("capture error: {e}");
                return AuthExitCode::InternalError;
            }
        };

        match broker::match_embedding_for_auth(username, auth_scope, embedding) {
            Ok(result) if result.matched => {
                matches += 1;
                tracing::debug!(
                    attempt,
                    matches,
                    required = config.recognition.required_matches,
                    score = result.score,
                    "face match accepted for attempt"
                );
                if matches >= config.recognition.required_matches {
                    tracing::info!("auth succeeded for '{username}'");
                    eprintln!("[ facegate ] \u{2714} Face recognized: {username}");
                    return AuthExitCode::Recognized;
                }
            }
            Ok(result) => {
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
        return timeout_or_deny(config);
    }
    fallback_or_deny(config)
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
            fallback_or_deny(config)
        }
        other => {
            tracing::error!("broker match error: {other}");
            AuthExitCode::InternalError
        }
    }
}
