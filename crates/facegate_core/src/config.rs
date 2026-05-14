use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{FaceRsError, Result};
use crate::storage::AuthScope;

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
    /// Secondary IR sensor used for the liveness cross-check. Has its own
    /// resolution / timeout / warmup defaults because IR modules typically
    /// stream lower-res GREY and take longer to settle than the colour sensor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ir: Option<CameraIrConfig>,
    #[serde(default)]
    pub cross_check: CameraCrossCheckConfig,
}

/// IR sensor capture settings. Every override field is optional and falls back
/// to an IR-friendly default at access time (see `CameraIrConfig::effective_*`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CameraIrConfig {
    pub device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warmup_frames: Option<u32>,
    /// Minimum bounding-box size (px) required to accept an IR face. Defaults
    /// lower than the RGB `recognition.min_face_size` because IR modules are
    /// typically lower-resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_face_size: Option<u32>,
}

impl CameraIrConfig {
    pub fn effective_width(&self, rgb_default: u32) -> u32 {
        self.width.unwrap_or(rgb_default)
    }
    pub fn effective_height(&self, rgb_default: u32) -> u32 {
        self.height.unwrap_or(rgb_default)
    }
    pub fn effective_fps(&self, rgb_default: u32) -> u32 {
        self.fps.unwrap_or(rgb_default)
    }
    pub fn effective_timeout_ms(&self, rgb_default: u64) -> u64 {
        self.timeout_ms.unwrap_or(rgb_default.max(8000))
    }
    pub fn effective_warmup_frames(&self, rgb_default: u32) -> u32 {
        self.warmup_frames.unwrap_or(rgb_default.max(10))
    }
    pub fn effective_min_face_size(&self, rgb_default: u32) -> u32 {
        self.min_face_size
            .unwrap_or_else(|| rgb_default.saturating_mul(5) / 8)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CameraCrossCheckConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_cross_check_max_time_skew_ms")]
    pub max_time_skew_ms: u64,
    #[serde(default = "default_cross_check_max_position_offset_px")]
    pub max_position_offset_px: f32,
    /// 3x3 projective homography mapping IR pixel coordinates into RGB pixel
    /// coordinates, stored **row-major**:
    /// `[a, b, tx, c, d, ty, g, h, 1]` corresponds to
    /// ```text
    ///     [ a  b  tx ]
    /// H = [ c  d  ty ]
    ///     [ g  h   1 ]
    /// ```
    /// The broker applies this to the IR landmark centroid before comparing
    /// against the RGB centroid. The default is the identity matrix; enabling
    /// cross-check while this is still the identity is refused at validation
    /// time unless `allow_identity_homography = true` (used only for
    /// pre-aligned sensors / tests).
    #[serde(default = "default_cross_check_homography")]
    pub homography: [f32; 9],
    /// Escape hatch for setups where the IR and RGB sensors are already
    /// pixel-aligned (rare on consumer laptops) or for tests. Without it,
    /// `validate()` refuses to enable cross-check while `homography` is still
    /// the identity matrix — most users need to run `facegate calibrate-cameras`
    /// first.
    #[serde(default)]
    pub allow_identity_homography: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecognitionConfig {
    pub threshold: f32,
    pub required_matches: u32,
    pub max_attempts: u32,
    pub min_face_size: u32,
    #[serde(default = "default_sudo_recognition_policy")]
    pub sudo: RecognitionScopeConfig,
    #[serde(default)]
    pub session: RecognitionScopeConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RecognitionScopeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_matches: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveRecognitionPolicy {
    pub threshold: f32,
    pub required_matches: u32,
    pub max_attempts: u32,
}

impl RecognitionConfig {
    pub fn policy_for(&self, scope: AuthScope) -> EffectiveRecognitionPolicy {
        let override_policy = match scope {
            AuthScope::Sudo => &self.sudo,
            AuthScope::Session => &self.session,
        };
        EffectiveRecognitionPolicy {
            threshold: override_policy.threshold.unwrap_or(self.threshold),
            required_matches: override_policy
                .required_matches
                .unwrap_or(self.required_matches),
            max_attempts: override_policy.max_attempts.unwrap_or(self.max_attempts),
        }
    }
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

fn default_sudo_recognition_policy() -> RecognitionScopeConfig {
    RecognitionScopeConfig {
        threshold: Some(0.60),
        required_matches: Some(2),
        max_attempts: Some(5),
    }
}

fn default_cross_check_max_time_skew_ms() -> u64 {
    // 50 ms was too aggressive on real hardware: the IR sensor frequently
    // takes 80–150 ms to deliver its first frame after STREAMON, especially
    // on Chicony 04f2:b829 and similar Windows-Hello modules. 200 ms keeps
    // the window tight enough to bound replay risk while letting honest
    // dual-camera captures through on the first attempt.
    200
}

fn default_cross_check_max_position_offset_px() -> f32 {
    40.0
}

fn default_cross_check_homography() -> [f32; 9] {
    [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
}

pub fn is_identity_homography(h: &[f32; 9]) -> bool {
    const IDENTITY: [f32; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    h.iter()
        .zip(IDENTITY.iter())
        .all(|(a, b)| (a - b).abs() < 1e-6)
}

impl Default for CameraCrossCheckConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_time_skew_ms: default_cross_check_max_time_skew_ms(),
            max_position_offset_px: default_cross_check_max_position_offset_px(),
            homography: default_cross_check_homography(),
            allow_identity_homography: false,
        }
    }
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
                ir: None,
                cross_check: CameraCrossCheckConfig::default(),
            },
            recognition: RecognitionConfig {
                threshold: 0.55,
                required_matches: 1,
                max_attempts: 3,
                min_face_size: 80,
                sudo: default_sudo_recognition_policy(),
                session: RecognitionScopeConfig::default(),
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
        if let Some(ir) = &self.camera.ir {
            if ir.device.trim().is_empty() {
                return Err(FaceRsError::Config(
                    "camera.ir.device cannot be empty when set".to_string(),
                ));
            }
            if ir.device == self.camera.device {
                return Err(FaceRsError::Config(
                    "camera.ir.device must be different from camera.device".to_string(),
                ));
            }
            if ir.width == Some(0) || ir.height == Some(0) {
                return Err(FaceRsError::Config(
                    "camera.ir width/height must be greater than zero".to_string(),
                ));
            }
            if ir.fps == Some(0) {
                return Err(FaceRsError::Config(
                    "camera.ir.fps must be greater than zero".to_string(),
                ));
            }
            if ir.timeout_ms == Some(0) {
                return Err(FaceRsError::Config(
                    "camera.ir.timeout_ms must be greater than zero".to_string(),
                ));
            }
            if ir.min_face_size == Some(0) {
                return Err(FaceRsError::Config(
                    "camera.ir.min_face_size must be greater than zero".to_string(),
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
        if self.camera.cross_check.enabled && self.camera.ir.is_none() {
            return Err(FaceRsError::Config(
                "camera.cross_check.enabled requires a [camera.ir] section".to_string(),
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
        if self.camera.cross_check.enabled
            && is_identity_homography(&self.camera.cross_check.homography)
            && !self.camera.cross_check.allow_identity_homography
        {
            return Err(FaceRsError::Config(
                "camera.cross_check.enabled is true but homography is still the identity matrix; \
                 run `facegate calibrate-cameras --write` first, or set \
                 camera.cross_check.allow_identity_homography = true if the sensors are physically aligned"
                    .to_string(),
            ));
        }
        if !self.recognition.threshold.is_finite()
            || !(0.0..=1.0).contains(&self.recognition.threshold)
        {
            return Err(FaceRsError::Config(
                "recognition.threshold must be a finite value between 0.0 and 1.0".to_string(),
            ));
        }
        validate_scope_recognition("recognition.sudo", &self.recognition.sudo)?;
        validate_scope_recognition("recognition.session", &self.recognition.session)?;
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
        for (label, scope) in [
            ("recognition.sudo", AuthScope::Sudo),
            ("recognition.session", AuthScope::Session),
        ] {
            let policy = self.recognition.policy_for(scope);
            if policy.required_matches > policy.max_attempts {
                return Err(FaceRsError::Config(format!(
                    "{label}.required_matches cannot exceed {label}.max_attempts"
                )));
            }
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

fn validate_scope_recognition(label: &str, policy: &RecognitionScopeConfig) -> Result<()> {
    if let Some(threshold) = policy.threshold {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Err(FaceRsError::Config(format!(
                "{label}.threshold must be a finite value between 0.0 and 1.0"
            )));
        }
    }
    if policy.required_matches == Some(0) {
        return Err(FaceRsError::Config(format!(
            "{label}.required_matches must be greater than zero"
        )));
    }
    if policy.max_attempts == Some(0) {
        return Err(FaceRsError::Config(format!(
            "{label}.max_attempts must be greater than zero"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let cfg = Config::default();
        assert_eq!(cfg.camera.device, "/dev/video0");
        assert!(cfg.camera.ir.is_none());
        assert!(!cfg.camera.cross_check.enabled);
        assert!(cfg.recognition.threshold > 0.0 && cfg.recognition.threshold < 1.0);
        assert!(cfg.security.allow_password_fallback);
        assert_eq!(cfg.security.cooldown_after_failures, 10);
        assert_eq!(cfg.security.cooldown_seconds, 60);
        assert_eq!(
            cfg.recognition.policy_for(AuthScope::Session).threshold,
            0.55
        );
        assert_eq!(cfg.recognition.policy_for(AuthScope::Sudo).threshold, 0.60);
        assert_eq!(
            cfg.recognition.policy_for(AuthScope::Sudo).required_matches,
            2
        );
        assert_eq!(cfg.recognition.policy_for(AuthScope::Sudo).max_attempts, 5);
        cfg.validate().expect("default config validates");
    }

    #[test]
    fn round_trip_toml() {
        let cfg = Config::default();
        let serialized = toml::to_string(&cfg).expect("serialize");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(cfg.camera.device, deserialized.camera.device);
        assert_eq!(cfg.camera.ir.is_some(), deserialized.camera.ir.is_some());
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
    fn rejects_enabled_cross_check_without_ir_section() {
        let mut cfg = Config::default();
        cfg.camera.cross_check.enabled = true;
        cfg.camera.cross_check.allow_identity_homography = true;
        assert!(cfg.validate().is_err());
        cfg.camera.ir = Some(CameraIrConfig {
            device: "/dev/video2".to_owned(),
            width: None,
            height: None,
            fps: None,
            timeout_ms: None,
            warmup_frames: None,
            min_face_size: None,
        });
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn rejects_identity_homography_when_cross_check_enabled() {
        let mut cfg = Config::default();
        cfg.camera.ir = Some(CameraIrConfig {
            device: "/dev/video2".to_owned(),
            width: None,
            height: None,
            fps: None,
            timeout_ms: None,
            warmup_frames: None,
            min_face_size: None,
        });
        cfg.camera.cross_check.enabled = true;
        // identity homography → refused unless the operator opts in
        assert!(cfg.validate().is_err());
        cfg.camera.cross_check.allow_identity_homography = true;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn rejects_invalid_cross_check_values() {
        let mut cfg = Config::default();
        cfg.camera.ir = Some(CameraIrConfig {
            device: "/dev/video2".to_owned(),
            width: None,
            height: None,
            fps: None,
            timeout_ms: None,
            warmup_frames: None,
            min_face_size: None,
        });
        cfg.camera.cross_check.enabled = true;
        cfg.camera.cross_check.allow_identity_homography = true;
        cfg.camera.cross_check.max_time_skew_ms = 0;
        assert!(cfg.validate().is_err());
        cfg.camera.cross_check.max_time_skew_ms = 50;
        cfg.camera.cross_check.max_position_offset_px = f32::NAN;
        assert!(cfg.validate().is_err());
        cfg.camera.cross_check.max_position_offset_px = 40.0;
        cfg.camera.cross_check.homography[8] = f32::NAN;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_impossible_match_policy() {
        let mut cfg = Config::default();
        cfg.recognition.required_matches = 2;
        cfg.recognition.max_attempts = 1;
        cfg.recognition.sudo = RecognitionScopeConfig::default();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn parses_legacy_recognition_config_with_scope_defaults() {
        let toml = r#"
            [camera]
            device = "/dev/video0"
            width = 640
            height = 360
            fps = 30
            timeout_ms = 5000
            warmup_frames = 3

            [recognition]
            threshold = 0.55
            required_matches = 1
            max_attempts = 3
            min_face_size = 80

            [models]
            detector = "/tmp/detector.onnx"
            embedder = "/tmp/embedder.onnx"

            [storage]
            base_dir = "/tmp/facegate/users"

            [logging]
            level = "info"
            log_failed_attempts = true

            [security]
            allow_password_fallback = true
            deny_on_camera_error = false
        "#;
        let cfg: Config = toml::from_str(toml).expect("legacy config parses");
        cfg.validate().expect("legacy config validates");
        assert_eq!(
            cfg.recognition.policy_for(AuthScope::Session).threshold,
            0.55
        );
        assert_eq!(cfg.recognition.policy_for(AuthScope::Sudo).threshold, 0.60);
    }

    #[test]
    fn scope_policy_overrides_global_defaults() {
        let mut cfg = Config::default();
        cfg.recognition.session.threshold = Some(0.58);
        cfg.recognition.session.required_matches = Some(2);
        cfg.recognition.session.max_attempts = Some(4);
        cfg.validate().expect("scope policy validates");
        assert_eq!(
            cfg.recognition.policy_for(AuthScope::Session),
            EffectiveRecognitionPolicy {
                threshold: 0.58,
                required_matches: 2,
                max_attempts: 4,
            }
        );
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
