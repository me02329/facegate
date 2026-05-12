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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ir_device: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub timeout_ms: u64,
    pub warmup_frames: u32,
    #[serde(default)]
    pub cross_check: CameraCrossCheckConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CameraCrossCheckConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cross_check_max_time_skew_ms")]
    pub max_time_skew_ms: u64,
    #[serde(default = "default_cross_check_max_position_offset_px")]
    pub max_position_offset_px: f32,
    #[serde(default = "default_cross_check_min_identity_similarity")]
    pub min_identity_similarity: f32,
    /// 3x3 projective homography mapping IR pixel coordinates into RGB pixel
    /// coordinates, stored **row-major**:
    /// `[a, b, tx, c, d, ty, g, h, 1]` corresponds to
    /// ```text
    ///     [ a  b  tx ]
    /// H = [ c  d  ty ]
    ///     [ g  h   1 ]
    /// ```
    /// The broker applies this to the IR landmark centroid before comparing
    /// against the RGB centroid. The default is the identity matrix, which is
    /// only correct when the two sensors are already pixel-aligned — real
    /// laptops with an IR module physically offset from the RGB camera need
    /// per-device calibration.
    #[serde(default = "default_cross_check_homography")]
    pub homography: [f32; 9],
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
    #[serde(default = "default_cooldown_after_failures")]
    pub cooldown_after_failures: u32,
    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u64,
}

fn default_cooldown_after_failures() -> u32 {
    10
}

fn default_cooldown_seconds() -> u64 {
    60
}

fn default_cross_check_max_time_skew_ms() -> u64 {
    50
}

fn default_cross_check_max_position_offset_px() -> f32 {
    40.0
}

fn default_cross_check_min_identity_similarity() -> f32 {
    0.55
}

fn default_cross_check_homography() -> [f32; 9] {
    [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
}

impl Default for CameraCrossCheckConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_time_skew_ms: default_cross_check_max_time_skew_ms(),
            max_position_offset_px: default_cross_check_max_position_offset_px(),
            min_identity_similarity: default_cross_check_min_identity_similarity(),
            homography: default_cross_check_homography(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            camera: CameraConfig {
                device: "/dev/video0".to_string(),
                ir_device: None,
                width: 640,
                height: 360,
                fps: 30,
                timeout_ms: 5000,
                warmup_frames: 3,
                cross_check: CameraCrossCheckConfig::default(),
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
                cooldown_after_failures: default_cooldown_after_failures(),
                cooldown_seconds: default_cooldown_seconds(),
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
        if let Some(ir_device) = &self.camera.ir_device {
            if ir_device.trim().is_empty() {
                return Err(FaceRsError::Config(
                    "camera.ir_device cannot be empty when set".to_string(),
                ));
            }
            if ir_device == &self.camera.device {
                return Err(FaceRsError::Config(
                    "camera.ir_device must be different from camera.device".to_string(),
                ));
            }
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
        if self.camera.cross_check.enabled && self.camera.ir_device.is_none() {
            return Err(FaceRsError::Config(
                "camera.cross_check.enabled requires camera.ir_device".to_string(),
            ));
        }
        if self.camera.cross_check.max_time_skew_ms == 0 {
            return Err(FaceRsError::Config(
                "camera.cross_check.max_time_skew_ms must be greater than zero".to_string(),
            ));
        }
        if !self.camera.cross_check.max_position_offset_px.is_finite()
            || self.camera.cross_check.max_position_offset_px <= 0.0
        {
            return Err(FaceRsError::Config(
                "camera.cross_check.max_position_offset_px must be a finite positive value"
                    .to_string(),
            ));
        }
        if !self.camera.cross_check.min_identity_similarity.is_finite()
            || !(0.0..=1.0).contains(&self.camera.cross_check.min_identity_similarity)
        {
            return Err(FaceRsError::Config(
                "camera.cross_check.min_identity_similarity must be a finite value between 0.0 and 1.0"
                    .to_string(),
            ));
        }
        if !self
            .camera
            .cross_check
            .homography
            .iter()
            .all(|v| v.is_finite())
        {
            return Err(FaceRsError::Config(
                "camera.cross_check.homography must contain only finite values".to_string(),
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
        if self.security.cooldown_after_failures == 0 {
            return Err(FaceRsError::Config(
                "security.cooldown_after_failures must be greater than zero".to_string(),
            ));
        }
        if self.security.cooldown_seconds == 0 {
            return Err(FaceRsError::Config(
                "security.cooldown_seconds must be greater than zero".to_string(),
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
        assert!(cfg.camera.ir_device.is_none());
        assert!(!cfg.camera.cross_check.enabled);
        assert!(cfg.recognition.threshold > 0.0 && cfg.recognition.threshold < 1.0);
        assert!(cfg.security.allow_password_fallback);
        assert_eq!(cfg.security.cooldown_after_failures, 10);
        assert_eq!(cfg.security.cooldown_seconds, 60);
        cfg.validate().expect("default config validates");
    }

    #[test]
    fn round_trip_toml() {
        let cfg = Config::default();
        let serialized = toml::to_string(&cfg).expect("serialize");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(cfg.camera.device, deserialized.camera.device);
        assert_eq!(cfg.camera.ir_device, deserialized.camera.ir_device);
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
    fn rejects_enabled_cross_check_without_ir_device() {
        let mut cfg = Config::default();
        cfg.camera.cross_check.enabled = true;
        assert!(cfg.validate().is_err());
        cfg.camera.ir_device = Some("/dev/video2".to_owned());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn rejects_invalid_cross_check_values() {
        let mut cfg = Config::default();
        cfg.camera.ir_device = Some("/dev/video2".to_owned());
        cfg.camera.cross_check.enabled = true;
        cfg.camera.cross_check.max_time_skew_ms = 0;
        assert!(cfg.validate().is_err());
        cfg.camera.cross_check.max_time_skew_ms = 50;
        cfg.camera.cross_check.max_position_offset_px = f32::NAN;
        assert!(cfg.validate().is_err());
        cfg.camera.cross_check.max_position_offset_px = 40.0;
        cfg.camera.cross_check.min_identity_similarity = 1.5;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_impossible_match_policy() {
        let mut cfg = Config::default();
        cfg.recognition.required_matches = 2;
        cfg.recognition.max_attempts = 1;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_invalid_cooldown_policy() {
        let mut cfg = Config::default();
        cfg.security.cooldown_after_failures = 0;
        assert!(cfg.validate().is_err());
        cfg.security.cooldown_after_failures = 10;
        cfg.security.cooldown_seconds = 0;
        assert!(cfg.validate().is_err());
    }
}
