use facegate_core::camera::V4lCamera;
use facegate_core::config::Config;
use facegate_core::error::{AuthExitCode, FaceRsError};
use facegate_core::storage::AuthScope;
use facegate_ipc::ErrorCode;

use crate::commands::broker;
use crate::commands::broker::frame_probe;

/// Non-interactive authentication called by the PAM module.
/// Returns an exit code — caller must pass it to std::process::exit.
///
/// Since broker-side MatchFrame the client only captures frames; SCRFD + ArcFace and the
/// match decision live inside facegate-brokerd, which means a same-UID
/// attacker cannot bypass live capture by submitting a precomputed embedding.
pub fn run(config: &Config, username: &str, service: Option<&str>) -> AuthExitCode {
    tracing::info!("auth requested for '{username}'");
    let auth_scope = auth_scope_for_service(service);
    let cross_check = cross_check_enabled(config);

    let mut camera = match V4lCamera::open(
        &config.camera.device,
        config.camera.width,
        config.camera.height,
        config.camera.fps,
        config.camera.timeout_ms,
    ) {
        Ok(mut cam) => {
            cam.warmup(config.camera.warmup_frames);
            cam
        }
        Err(FaceRsError::Camera(msg)) => {
            eprintln!("Facegate: camera error: {msg}");
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
        let Some(ir_device) = config.camera.ir_device.as_deref() else {
            tracing::error!("cross-check is enabled but camera.ir_device is missing");
            return AuthExitCode::InternalError;
        };
        match V4lCamera::open(
            ir_device,
            config.camera.width,
            config.camera.height,
            config.camera.fps,
            config.camera.timeout_ms,
        ) {
            Ok(mut cam) => {
                cam.warmup(config.camera.warmup_frames);
                Some(cam)
            }
            Err(FaceRsError::Camera(msg)) => {
                eprintln!("Facegate: IR camera error: {msg}");
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
    for attempt in 1..=config.recognition.max_attempts {
        let frame = match camera.capture_frame() {
            Ok(f) => f,
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
            Err(e) => {
                tracing::error!("capture error: {e}");
                return AuthExitCode::InternalError;
            }
        };

        let result = if let Some(ir_camera) = ir_camera.as_mut() {
            let rgb_probe = frame_probe(frame);
            let ir_frame = match ir_camera.capture_frame() {
                Ok(f) => f,
                Err(FaceRsError::Timeout) => {
                    saw_timeout = true;
                    tracing::warn!(attempt, "IR face auth timed out for '{username}'");
                    continue;
                }
                Err(FaceRsError::Camera(msg)) => {
                    eprintln!("Facegate: IR camera error during capture: {msg}");
                    return if config.security.deny_on_camera_error {
                        AuthExitCode::CameraError
                    } else {
                        fallback_or_deny(config)
                    };
                }
                Err(e) => {
                    tracing::error!("IR capture error: {e}");
                    return AuthExitCode::InternalError;
                }
            };
            let ir_probe = frame_probe(ir_frame);
            broker::match_frame_pair_for_auth(username, auth_scope, rgb_probe, ir_probe)
        } else {
            broker::match_frame_for_auth(username, auth_scope, frame_probe(frame))
        };

        match result {
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

fn cross_check_enabled(config: &Config) -> bool {
    config.camera.cross_check.enabled && config.camera.ir_device.is_some()
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
