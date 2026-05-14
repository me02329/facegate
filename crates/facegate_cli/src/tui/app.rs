use anyhow::anyhow;
use facegate_core::config::{
    CameraConfig, CameraCrossCheckConfig, CameraIrConfig, Config, LoggingConfig, ModelsConfig,
    RecognitionConfig, SecurityConfig, StorageConfig,
};
use std::path::PathBuf;

use crate::commands::services;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sections,
    Fields,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Editing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureExit {
    Back,
    Quit,
}

pub struct Field {
    pub key: &'static str,
    pub description: &'static str,
    pub value: String,
}

pub struct Section {
    pub name: &'static str,
    pub fields: Vec<Field>,
}

pub struct App {
    pub config: Config,
    pub config_path: PathBuf,
    pub sections: Vec<Section>,
    pub focus: Focus,
    pub selected_section: usize,
    pub selected_field: usize,
    pub mode: Mode,
    pub edit_buffer: String,
    /// (message, is_error)
    pub status: Option<(String, bool)>,
    pub exit: Option<ConfigureExit>,
}

impl App {
    pub fn new(config: Config, config_path: PathBuf) -> Self {
        let sections = build_sections(&config);
        App {
            config,
            config_path,
            sections,
            focus: Focus::Sections,
            selected_section: 0,
            selected_field: 0,
            mode: Mode::Normal,
            edit_buffer: String::new(),
            status: None,
            exit: None,
        }
    }

    pub fn move_up(&mut self) {
        self.status = None;
        match self.focus {
            Focus::Sections => {
                if self.selected_section > 0 {
                    self.selected_section -= 1;
                    self.selected_field = 0;
                }
            }
            Focus::Fields => {
                if self.selected_field > 0 {
                    self.selected_field -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        self.status = None;
        match self.focus {
            Focus::Sections => {
                if self.selected_section < self.sections.len() - 1 {
                    self.selected_section += 1;
                    self.selected_field = 0;
                }
            }
            Focus::Fields => {
                let max = self.sections[self.selected_section].fields.len();
                if self.selected_field < max - 1 {
                    self.selected_field += 1;
                }
            }
        }
    }

    pub fn enter(&mut self) {
        match self.focus {
            Focus::Sections => {
                self.focus = Focus::Fields;
                self.selected_field = 0;
            }
            Focus::Fields => self.start_edit(),
        }
    }

    fn start_edit(&mut self) {
        let value = self.sections[self.selected_section].fields[self.selected_field]
            .value
            .clone();
        self.edit_buffer = value;
        self.mode = Mode::Editing;
        self.status = None;
    }

    pub fn confirm_edit(&mut self) {
        let s = self.selected_section;
        let f = self.selected_field;
        self.sections[s].fields[f].value = self.edit_buffer.clone();
        self.mode = Mode::Normal;
        self.edit_buffer.clear();
    }

    pub fn cancel_edit(&mut self) {
        self.mode = Mode::Normal;
        self.edit_buffer.clear();
    }

    pub fn save(&mut self) {
        match sections_to_config(&self.sections, &self.config) {
            Ok(new_config) => match new_config.validate() {
                Ok(()) => match toml::to_string_pretty(&new_config) {
                    Ok(s) => match std::fs::write(&self.config_path, s) {
                        Ok(()) => {
                            self.config = new_config;
                            let refresh = services::refresh_after_config_change();
                            self.status = Some((
                                format!(
                                    "✓  saved to {} | broker={} watch={}",
                                    self.config_path.display(),
                                    refresh.broker.label(),
                                    refresh.watch.label()
                                ),
                                false,
                            ));
                        }
                        Err(e) => {
                            self.status = Some((format!("✗  write failed: {e}"), true));
                        }
                    },
                    Err(e) => {
                        self.status = Some((format!("✗  serialize error: {e}"), true));
                    }
                },
                Err(e) => {
                    self.status = Some((format!("✗  invalid config: {e}"), true));
                }
            },
            Err(e) => {
                self.status = Some((format!("✗  invalid value: {e}"), true));
            }
        }
    }
}

// ── Build sections from Config ────────────────────────────────────────────────

fn build_sections(cfg: &Config) -> Vec<Section> {
    vec![
        Section {
            name: "Camera",
            fields: vec![
                Field {
                    key: "device",
                    description: "Primary RGB camera device path (e.g. /dev/video0)",
                    value: cfg.camera.device.clone(),
                },
                Field {
                    key: "width",
                    description: "Capture width in pixels",
                    value: cfg.camera.width.to_string(),
                },
                Field {
                    key: "height",
                    description: "Capture height in pixels",
                    value: cfg.camera.height.to_string(),
                },
                Field {
                    key: "fps",
                    description: "Frames per second",
                    value: cfg.camera.fps.to_string(),
                },
                Field {
                    key: "timeout_ms",
                    description: "Authentication timeout in milliseconds",
                    value: cfg.camera.timeout_ms.to_string(),
                },
                Field {
                    key: "warmup_frames",
                    description: "Frames to discard before capturing",
                    value: cfg.camera.warmup_frames.to_string(),
                },
                Field {
                    key: "ir_device",
                    description: "IR camera device path for cross-check (leave empty to disable)",
                    value: cfg
                        .camera
                        .ir
                        .as_ref()
                        .map(|ir| ir.device.clone())
                        .unwrap_or_default(),
                },
                Field {
                    key: "ir_width",
                    description: "IR capture width override (blank = inherit RGB width)",
                    value: cfg
                        .camera
                        .ir
                        .as_ref()
                        .and_then(|ir| ir.width)
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                },
                Field {
                    key: "ir_height",
                    description: "IR capture height override (blank = inherit)",
                    value: cfg
                        .camera
                        .ir
                        .as_ref()
                        .and_then(|ir| ir.height)
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                },
                Field {
                    key: "ir_timeout_ms",
                    description: "IR capture timeout override (blank = max(rgb, 8000))",
                    value: cfg
                        .camera
                        .ir
                        .as_ref()
                        .and_then(|ir| ir.timeout_ms)
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                },
                Field {
                    key: "ir_warmup_frames",
                    description: "IR warmup frames override (blank = max(rgb, 10))",
                    value: cfg
                        .camera
                        .ir
                        .as_ref()
                        .and_then(|ir| ir.warmup_frames)
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                },
                Field {
                    key: "ir_min_face_size",
                    description: "Minimum IR face size in px (blank = 5/8 of RGB min_face_size)",
                    value: cfg
                        .camera
                        .ir
                        .as_ref()
                        .and_then(|ir| ir.min_face_size)
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                },
                Field {
                    key: "cross_check_enabled",
                    description: "Require synchronized RGB+IR cross-check (true/false)",
                    value: cfg.camera.cross_check.enabled.to_string(),
                },
                Field {
                    key: "cross_check_max_time_skew_ms",
                    description: "Maximum RGB/IR capture timestamp skew in milliseconds",
                    value: cfg.camera.cross_check.max_time_skew_ms.to_string(),
                },
                Field {
                    key: "cross_check_max_position_offset_px",
                    description: "Maximum mapped RGB/IR landmark centroid offset in pixels",
                    value: cfg.camera.cross_check.max_position_offset_px.to_string(),
                },
                Field {
                    key: "cross_check_allow_identity_homography",
                    description: "Allow enabling cross-check with the identity homography (sensors must be pre-aligned)",
                    value: cfg.camera.cross_check.allow_identity_homography.to_string(),
                },
            ],
        },
        Section {
            name: "Recognition",
            fields: vec![
                Field {
                    key: "threshold",
                    description: "Cosine similarity threshold [0.0 – 1.0]",
                    value: cfg.recognition.threshold.to_string(),
                },
                Field {
                    key: "required_matches",
                    description: "Minimum number of matches required",
                    value: cfg.recognition.required_matches.to_string(),
                },
                Field {
                    key: "max_attempts",
                    description: "Max capture attempts before giving up",
                    value: cfg.recognition.max_attempts.to_string(),
                },
                Field {
                    key: "min_face_size",
                    description: "Minimum face bounding-box size in pixels",
                    value: cfg.recognition.min_face_size.to_string(),
                },
            ],
        },
        Section {
            name: "Models",
            fields: vec![
                Field {
                    key: "detector",
                    description: "Path to SCRFD face detector ONNX model",
                    value: cfg.models.detector.to_string_lossy().into_owned(),
                },
                Field {
                    key: "embedder",
                    description: "Path to ArcFace embedder ONNX model",
                    value: cfg.models.embedder.to_string_lossy().into_owned(),
                },
            ],
        },
        Section {
            name: "Storage",
            fields: vec![Field {
                key: "base_dir",
                description: "Root directory for per-user template files",
                value: cfg.storage.base_dir.to_string_lossy().into_owned(),
            }],
        },
        Section {
            name: "Logging",
            fields: vec![
                Field {
                    key: "level",
                    description: "Log level: trace / debug / info / warn / error",
                    value: cfg.logging.level.clone(),
                },
                Field {
                    key: "log_failed_attempts",
                    description: "Log failed authentication attempts (true/false)",
                    value: cfg.logging.log_failed_attempts.to_string(),
                },
            ],
        },
        Section {
            name: "Security",
            fields: vec![
                Field {
                    key: "allow_password_fallback",
                    description: "Allow password fallback when face auth fails (true/false)",
                    value: cfg.security.allow_password_fallback.to_string(),
                },
                Field {
                    key: "deny_on_camera_error",
                    description: "Deny authentication if camera cannot be opened (true/false)",
                    value: cfg.security.deny_on_camera_error.to_string(),
                },
            ],
        },
    ]
}

// ── Apply sections back to Config ─────────────────────────────────────────────

fn sections_to_config(sections: &[Section], base: &Config) -> anyhow::Result<Config> {
    Ok(Config {
        camera: parse_camera(&sections[0], &base.camera)?,
        recognition: parse_recognition(&sections[1], &base.recognition)?,
        models: parse_models(&sections[2], &base.models)?,
        storage: parse_storage(&sections[3], &base.storage)?,
        logging: parse_logging(&sections[4], &base.logging)?,
        security: parse_security(&sections[5], &base.security)?,
    })
}

fn parse_camera(section: &Section, base: &CameraConfig) -> anyhow::Result<CameraConfig> {
    let mut cfg = base.clone();
    // Start from whatever IR config the operator already had; we only keep it
    // if the device field is still non-empty after the edit.
    let mut ir_device: Option<String> = cfg.ir.as_ref().map(|ir| ir.device.clone());
    let mut ir_width: Option<u32> = cfg.ir.as_ref().and_then(|ir| ir.width);
    let mut ir_height: Option<u32> = cfg.ir.as_ref().and_then(|ir| ir.height);
    let mut ir_timeout_ms: Option<u64> = cfg.ir.as_ref().and_then(|ir| ir.timeout_ms);
    let mut ir_warmup: Option<u32> = cfg.ir.as_ref().and_then(|ir| ir.warmup_frames);
    let mut ir_min_face_size: Option<u32> = cfg.ir.as_ref().and_then(|ir| ir.min_face_size);
    let ir_fps: Option<u32> = cfg.ir.as_ref().and_then(|ir| ir.fps);

    for f in &section.fields {
        match f.key {
            "device" => cfg.device = f.value.clone(),
            "width" => cfg.width = parse_u32(&f.value, f.key)?,
            "height" => cfg.height = parse_u32(&f.value, f.key)?,
            "fps" => cfg.fps = parse_u32(&f.value, f.key)?,
            "timeout_ms" => cfg.timeout_ms = parse_u64(&f.value, f.key)?,
            "warmup_frames" => cfg.warmup_frames = parse_u32(&f.value, f.key)?,
            "ir_device" => {
                let trimmed = f.value.trim();
                ir_device = (!trimmed.is_empty()).then(|| trimmed.to_owned());
            }
            "ir_width" => ir_width = parse_optional_u32(&f.value, f.key)?,
            "ir_height" => ir_height = parse_optional_u32(&f.value, f.key)?,
            "ir_timeout_ms" => ir_timeout_ms = parse_optional_u64(&f.value, f.key)?,
            "ir_warmup_frames" => ir_warmup = parse_optional_u32(&f.value, f.key)?,
            "ir_min_face_size" => ir_min_face_size = parse_optional_u32(&f.value, f.key)?,
            "cross_check_enabled" => cfg.cross_check.enabled = parse_bool(&f.value, f.key)?,
            "cross_check_max_time_skew_ms" => {
                cfg.cross_check.max_time_skew_ms = parse_u64(&f.value, f.key)?
            }
            "cross_check_max_position_offset_px" => {
                cfg.cross_check.max_position_offset_px = parse_f32(&f.value, f.key)?
            }
            "cross_check_allow_identity_homography" => {
                cfg.cross_check.allow_identity_homography = parse_bool(&f.value, f.key)?
            }
            _ => {}
        }
    }
    cfg.ir = ir_device.map(|device| CameraIrConfig {
        device,
        width: ir_width,
        height: ir_height,
        fps: ir_fps,
        timeout_ms: ir_timeout_ms,
        warmup_frames: ir_warmup,
        min_face_size: ir_min_face_size,
    });
    fill_cross_check_defaults(&mut cfg.cross_check);
    Ok(cfg)
}

fn fill_cross_check_defaults(cfg: &mut CameraCrossCheckConfig) {
    let defaults = CameraCrossCheckConfig::default();
    if cfg.max_time_skew_ms == 0 {
        cfg.max_time_skew_ms = defaults.max_time_skew_ms;
    }
    if cfg.max_position_offset_px == 0.0 {
        cfg.max_position_offset_px = defaults.max_position_offset_px;
    }
}

fn parse_recognition(
    section: &Section,
    base: &RecognitionConfig,
) -> anyhow::Result<RecognitionConfig> {
    let mut cfg = base.clone();
    for f in &section.fields {
        match f.key {
            "threshold" => cfg.threshold = parse_f32(&f.value, f.key)?,
            "required_matches" => cfg.required_matches = parse_u32(&f.value, f.key)?,
            "max_attempts" => cfg.max_attempts = parse_u32(&f.value, f.key)?,
            "min_face_size" => cfg.min_face_size = parse_u32(&f.value, f.key)?,
            _ => {}
        }
    }
    Ok(cfg)
}

fn parse_models(section: &Section, base: &ModelsConfig) -> anyhow::Result<ModelsConfig> {
    let mut cfg = base.clone();
    for f in &section.fields {
        match f.key {
            "detector" => cfg.detector = PathBuf::from(&f.value),
            "embedder" => cfg.embedder = PathBuf::from(&f.value),
            _ => {}
        }
    }
    Ok(cfg)
}

fn parse_storage(section: &Section, base: &StorageConfig) -> anyhow::Result<StorageConfig> {
    let mut cfg = base.clone();
    for f in &section.fields {
        if f.key == "base_dir" {
            cfg.base_dir = PathBuf::from(&f.value);
        }
    }
    Ok(cfg)
}

fn parse_logging(section: &Section, base: &LoggingConfig) -> anyhow::Result<LoggingConfig> {
    let mut cfg = base.clone();
    for f in &section.fields {
        match f.key {
            "level" => cfg.level = f.value.clone(),
            "log_failed_attempts" => cfg.log_failed_attempts = parse_bool(&f.value, f.key)?,
            _ => {}
        }
    }
    Ok(cfg)
}

fn parse_security(section: &Section, base: &SecurityConfig) -> anyhow::Result<SecurityConfig> {
    let mut cfg = base.clone();
    for f in &section.fields {
        match f.key {
            "allow_password_fallback" => cfg.allow_password_fallback = parse_bool(&f.value, f.key)?,
            "deny_on_camera_error" => cfg.deny_on_camera_error = parse_bool(&f.value, f.key)?,
            _ => {}
        }
    }
    Ok(cfg)
}

fn parse_u32(s: &str, key: &str) -> anyhow::Result<u32> {
    s.trim()
        .parse()
        .map_err(|_| anyhow!("'{key}' expects a positive integer, got '{s}'"))
}

fn parse_u64(s: &str, key: &str) -> anyhow::Result<u64> {
    s.trim()
        .parse()
        .map_err(|_| anyhow!("'{key}' expects a positive integer, got '{s}'"))
}

fn parse_f32(s: &str, key: &str) -> anyhow::Result<f32> {
    s.trim()
        .parse()
        .map_err(|_| anyhow!("'{key}' expects a decimal number, got '{s}'"))
}

fn parse_bool(s: &str, key: &str) -> anyhow::Result<bool> {
    match s.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(anyhow!("'{key}' expects true or false, got '{other}'")),
    }
}

fn parse_optional_u32(s: &str, key: &str) -> anyhow::Result<Option<u32>> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(parse_u32(trimmed, key)?))
}

fn parse_optional_u64(s: &str, key: &str) -> anyhow::Result<Option<u64>> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(parse_u64(trimmed, key)?))
}
