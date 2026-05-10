use facegate_core::config::Config;
use facegate_core::error::{AuthExitCode, FaceRsError};
use facegate_core::matching::is_match;
use facegate_core::pipeline::FacePipeline;
use facegate_core::storage::{AuthScope, TemplateStore};

/// Non-interactive authentication called by the PAM module.
/// Returns an exit code — caller must pass it to std::process::exit.
pub fn run(config: &Config, username: &str, service: Option<&str>) -> AuthExitCode {
    tracing::info!("auth requested for '{username}'");
    let auth_scope = auth_scope_for_service(service);

    // Load enrolled templates first — cheap check before opening camera.
    let store = TemplateStore::new(&config.storage.base_dir);
    let enrolled = match store.embeddings_for_scope(username, auth_scope) {
        Ok(e) => e,
        Err(FaceRsError::NotEnrolled) => {
            tracing::warn!("'{username}' has no enrolled templates");
            return fallback_or_deny(config);
        }
        Err(e) => {
            tracing::error!("storage error: {e}");
            return AuthExitCode::InternalError;
        }
    };

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
        Err(e) => {
            tracing::error!("pipeline init error: {e}");
            return AuthExitCode::InternalError;
        }
    };

    let threshold = config.recognition.threshold;
    let mut matches = 0_u32;
    for attempt in 1..=config.recognition.max_attempts {
        let embedding = match pipeline.capture_embedding(config) {
            Ok(e) => e,
            Err(FaceRsError::Timeout) => {
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
            Err(e) => {
                tracing::error!("capture error: {e}");
                return AuthExitCode::InternalError;
            }
        };

        if is_match(&embedding, &enrolled, threshold) {
            matches += 1;
            tracing::debug!(
                attempt,
                matches,
                required = config.recognition.required_matches,
                "face match accepted for attempt"
            );
            if matches >= config.recognition.required_matches {
                tracing::info!("auth succeeded for '{username}'");
                eprintln!("[ facegate ] \u{2714} Face recognized : {username}");
                return AuthExitCode::Recognized;
            }
        } else if config.logging.log_failed_attempts {
            tracing::warn!(attempt, "auth failed for '{username}': face not recognised");
        }
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
