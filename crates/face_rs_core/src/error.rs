use thiserror::Error;

#[derive(Debug, Error)]
pub enum FaceRsError {
    #[error("camera error: {0}")]
    Camera(String),
    #[error("face detection error: {0}")]
    Detection(String),
    #[error("embedding error: {0}")]
    Embedding(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("no face detected within timeout")]
    NoFace,
    #[error("authentication timed out")]
    Timeout,
    #[error("user has no enrolled templates")]
    NotEnrolled,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, FaceRsError>;

/// Exit codes returned by `face-rs auth`, consumed by the PAM module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum AuthExitCode {
    Recognized = 0,
    NotRecognized = 1,
    Timeout = 2,
    CameraError = 3,
    ConfigError = 4,
    InternalError = 5,
    Denied = 6,
}

impl From<&FaceRsError> for AuthExitCode {
    fn from(e: &FaceRsError) -> Self {
        match e {
            FaceRsError::Timeout => AuthExitCode::Timeout,
            FaceRsError::NoFace => AuthExitCode::NotRecognized,
            FaceRsError::NotEnrolled => AuthExitCode::NotRecognized,
            FaceRsError::Camera(_) => AuthExitCode::CameraError,
            FaceRsError::Config(_) => AuthExitCode::ConfigError,
            _ => AuthExitCode::InternalError,
        }
    }
}
