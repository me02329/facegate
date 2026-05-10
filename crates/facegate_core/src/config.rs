use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{FaceRsError, Result};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/facegate/config.toml";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub camera: CameraConfig,
    pub recognition: RecognitionConfig,
    pub models: ModelsConfig,
    pub storage: StorageConfig,
    pub logging: LoggingConfig,
    pub security: SecurityConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CameraConfig {
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub timeout_ms: u64,
    pub warmup_frames: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecognitionConfig {
    pub threshold: f32,
    pub required_matches: u32,
    pub max_attempts: u32,
    pub min_face_size: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelsConfig {
    pub detector: PathBuf,
    pub embedder: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StorageConfig {
    pub base_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    pub level: String,
    pub log_failed_attempts: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SecurityConfig {
    pub allow_password_fallback: bool,
    pub deny_on_camera_error: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            camera: CameraConfig {
                device: "/dev/video0".to_string(),
                width: 640,
                height: 360,
                fps: 30,
                timeout_ms: 5000,
                warmup_frames: 3,
            },
            recognition: RecognitionConfig {
                threshold: 0.55,
                required_matches: 1,
                max_attempts: 3,
                min_face_size: 80,
            },
            models: ModelsConfig {
                detector: PathBuf::from("/usr/share/facegate/models/scrfd_500m.onnx"),
                embedder: PathBuf::from("/usr/share/facegate/models/arcface_w600k_r50.onnx"),
            },
            storage: StorageConfig {
                base_dir: PathBuf::from("/var/lib/facegate/users"),
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                log_failed_attempts: true,
            },
            security: SecurityConfig {
                allow_password_fallback: true,
                deny_on_camera_error: false,
            },
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| FaceRsError::Config(format!("cannot read {}: {e}", path.display())))?;
        let config: Self = toml::from_str(&content).map_err(|e| {
            FaceRsError::Config(format!("invalid config at {}: {e}", path.display()))
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn load_default() -> Result<Self> {
        Self::load(Path::new(DEFAULT_CONFIG_PATH))
    }

    pub fn load_or_default() -> Self {
        Self::load_default().unwrap_or_default()
    }

    pub fn validate(&self) -> Result<()> {
        if self.camera.device.trim().is_empty() {
            return Err(FaceRsError::Config(
                "camera.device cannot be empty".to_string(),
            ));
        }
        if self.camera.width == 0 || self.camera.height == 0 {
            return Err(FaceRsError::Config(
                "camera width and height must be greater than zero".to_string(),
            ));
        }
        if self.camera.fps == 0 {
            return Err(FaceRsError::Config(
                "camera.fps must be greater than zero".to_string(),
            ));
        }
        if self.camera.timeout_ms == 0 {
            return Err(FaceRsError::Config(
                "camera.timeout_ms must be greater than zero".to_string(),
            ));
        }
        if !self.recognition.threshold.is_finite()
            || !(0.0..=1.0).contains(&self.recognition.threshold)
        {
            return Err(FaceRsError::Config(
                "recognition.threshold must be a finite value between 0.0 and 1.0".to_string(),
            ));
        }
        if self.recognition.required_matches == 0 {
            return Err(FaceRsError::Config(
                "recognition.required_matches must be greater than zero".to_string(),
            ));
        }
        if self.recognition.max_attempts == 0 {
            return Err(FaceRsError::Config(
                "recognition.max_attempts must be greater than zero".to_string(),
            ));
        }
        if self.recognition.required_matches > self.recognition.max_attempts {
            return Err(FaceRsError::Config(
                "recognition.required_matches cannot exceed recognition.max_attempts".to_string(),
            ));
        }
        if self.recognition.min_face_size == 0 {
            return Err(FaceRsError::Config(
                "recognition.min_face_size must be greater than zero".to_string(),
            ));
        }
        if self.models.detector.as_os_str().is_empty()
            || self.models.embedder.as_os_str().is_empty()
        {
            return Err(FaceRsError::Config(
                "model paths cannot be empty".to_string(),
            ));
        }
        if self.storage.base_dir.as_os_str().is_empty() {
            return Err(FaceRsError::Config(
                "storage.base_dir cannot be empty".to_string(),
            ));
        }
        match self.logging.level.as_str() {
            "trace" | "debug" | "info" | "warn" | "error" => {}
            other => {
                return Err(FaceRsError::Config(format!(
                    "logging.level must be trace, debug, info, warn, or error; got '{other}'"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let cfg = Config::default();
        assert_eq!(cfg.camera.device, "/dev/video0");
        assert!(cfg.recognition.threshold > 0.0 && cfg.recognition.threshold < 1.0);
        assert!(cfg.security.allow_password_fallback);
        cfg.validate().expect("default config validates");
    }

    #[test]
    fn round_trip_toml() {
        let cfg = Config::default();
        let serialized = toml::to_string(&cfg).expect("serialize");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(cfg.camera.device, deserialized.camera.device);
        assert_eq!(
            cfg.recognition.threshold,
            deserialized.recognition.threshold
        );
    }

    #[test]
    fn rejects_unsafe_threshold() {
        let mut cfg = Config::default();
        cfg.recognition.threshold = -0.1;
        assert!(cfg.validate().is_err());
        cfg.recognition.threshold = f32::NAN;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_zero_capture_values() {
        let mut cfg = Config::default();
        cfg.camera.fps = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_impossible_match_policy() {
        let mut cfg = Config::default();
        cfg.recognition.required_matches = 2;
        cfg.recognition.max_attempts = 1;
        assert!(cfg.validate().is_err());
    }
}
