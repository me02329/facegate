use anyhow::{bail, Result};
use facegate_core::camera::{Frame, V4lCamera};
use facegate_core::config::Config;
use facegate_core::error::FaceRsError;
use facegate_core::storage::{AuthScope, EnrolledTemplate, TemplateScope};
use facegate_ipc::{
    send_request, AuditEvent, BrokerError, EnrolledTemplateSummary, EnrolledUserSummary, ErrorCode,
    FrameFormat, FrameProbe, MatchReason, MatchResult, Request, RequestEnvelope, Response,
    DEFAULT_SOCKET_PATH,
};

/// Wrap a freshly captured `Frame` in a `FrameProbe`. The capture timestamp
/// was stamped by the camera layer at dequeue time (`Frame::captured_at_ms`),
/// not at submission time, so the broker's RGB+IR sync window measures the
/// actual capture skew. `Frame::data` is always RGB24 (the camera layer
/// converts YUYV / MJPEG / GREY → RGB before returning), so the format is
/// always `Rgb8`.
pub fn frame_probe(frame: Frame) -> FrameProbe {
    FrameProbe {
        format: FrameFormat::Rgb8,
        width: frame.width,
        height: frame.height,
        captured_at_ms: frame.captured_at_ms,
        bytes: frame.data,
    }
}

pub fn match_embedding(
    username: &str,
    auth_scope: AuthScope,
    probe_embedding: Vec<f32>,
) -> Result<MatchResult> {
    match request(match_request(username, auth_scope, probe_embedding))? {
        Response::Match { result } => Ok(result),
        other => bail!("unexpected broker response: {other:?}"),
    }
}

pub fn match_frame(
    username: &str,
    auth_scope: AuthScope,
    frame: FrameProbe,
) -> Result<MatchResult> {
    let request = Request::MatchFrame {
        username: username.to_owned(),
        auth_scope: ipc_auth_scope(auth_scope),
        frame,
    };
    match self::request(request)? {
        Response::Match { result } => Ok(result),
        other => bail!("unexpected broker response: {other:?}"),
    }
}

pub fn match_frame_pair(
    username: &str,
    auth_scope: AuthScope,
    rgb_frame: FrameProbe,
    ir_frame: FrameProbe,
) -> Result<MatchResult> {
    let request = Request::MatchFramePair {
        username: username.to_owned(),
        auth_scope: ipc_auth_scope(auth_scope),
        rgb_frame,
        ir_frame,
    };
    match self::request(request)? {
        Response::Match { result } => Ok(result),
        other => bail!("unexpected broker response: {other:?}"),
    }
}

/// Submit a raw frame to the broker for detection + embedding + match. This
/// is the trust-bounded auth path: the client never computes the embedding,
/// so a same-UID attacker cannot bypass live capture by feeding a synthetic
/// vector. Used by `facegate auth` (PAM) and `facegate watch`.
pub fn match_frame_for_auth(
    username: &str,
    auth_scope: AuthScope,
    frame: FrameProbe,
) -> std::result::Result<MatchResult, BrokerAuthError> {
    let response = send_request(
        DEFAULT_SOCKET_PATH,
        RequestEnvelope::new(Request::MatchFrame {
            username: username.to_owned(),
            auth_scope: ipc_auth_scope(auth_scope),
            frame,
        }),
    )?;
    match response.response {
        Response::Match { result } => Ok(result),
        Response::Error(error) => Err(BrokerAuthError::Broker(error)),
        other => Err(BrokerAuthError::Unexpected(format!("{other:?}"))),
    }
}

pub fn match_frame_pair_for_auth(
    username: &str,
    auth_scope: AuthScope,
    rgb_frame: FrameProbe,
    ir_frame: FrameProbe,
) -> std::result::Result<MatchResult, BrokerAuthError> {
    let response = send_request(
        DEFAULT_SOCKET_PATH,
        RequestEnvelope::new(Request::MatchFramePair {
            username: username.to_owned(),
            auth_scope: ipc_auth_scope(auth_scope),
            rgb_frame,
            ir_frame,
        }),
    )?;
    match response.response {
        Response::Match { result } => Ok(result),
        Response::Error(error) => Err(BrokerAuthError::Broker(error)),
        other => Err(BrokerAuthError::Unexpected(format!("{other:?}"))),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BrokerAuthError {
    #[error(transparent)]
    Client(#[from] facegate_ipc::ClientError),
    #[error("broker error {:?}: {}", .0.code, .0.message)]
    Broker(BrokerError),
    #[error("unexpected broker response: {0}")]
    Unexpected(String),
}

fn match_request(username: &str, auth_scope: AuthScope, probe_embedding: Vec<f32>) -> Request {
    Request::Match {
        username: username.to_owned(),
        auth_scope: ipc_auth_scope(auth_scope),
        probe_embedding,
    }
}

pub fn enroll_template(
    username: &str,
    label: &str,
    scope: TemplateScope,
    embedding: Vec<f32>,
) -> Result<EnrolledTemplateSummary> {
    match request(Request::Enroll {
        username: username.to_owned(),
        label: label.to_owned(),
        scope: ipc_template_scope(scope),
        embedding,
    })? {
        Response::Enrolled { template } => Ok(template),
        other => bail!("unexpected broker response: {other:?}"),
    }
}

pub fn list_templates(username: &str) -> Result<Vec<EnrolledTemplateSummary>> {
    match request(Request::List {
        username: username.to_owned(),
    })? {
        Response::List { templates } => Ok(templates),
        other => bail!("unexpected broker response: {other:?}"),
    }
}

pub fn list_users() -> Result<Vec<EnrolledUserSummary>> {
    match request(Request::Users)? {
        Response::Users { users } => Ok(users),
        other => bail!("unexpected broker response: {other:?}"),
    }
}

pub fn remove_template(username: &str, template_id: u32) -> Result<()> {
    match request(Request::Remove {
        username: username.to_owned(),
        template_id,
    })? {
        Response::Removed => Ok(()),
        other => bail!("unexpected broker response: {other:?}"),
    }
}

pub fn audit_recent(username: Option<String>, limit: u32) -> Result<Vec<AuditEvent>> {
    match request(Request::AuditRecent { username, limit })? {
        Response::Audit { events } => Ok(events),
        other => bail!("unexpected broker response: {other:?}"),
    }
}

pub fn match_reason_label(reason: MatchReason) -> &'static str {
    match reason {
        MatchReason::Matched => "matched",
        MatchReason::TemplateMismatch => "template_mismatch",
        MatchReason::NotEnrolled => "not_enrolled",
        MatchReason::NoFace => "no_face",
        MatchReason::MultipleFaces => "multiple_faces",
        MatchReason::CrossCheckRequired => "cross_check_required",
        MatchReason::CrossCheckTimeSkew => "cross_check_time_skew",
        MatchReason::CrossCheckRgbNoFace => "cross_check_rgb_no_face",
        MatchReason::CrossCheckRgbMultipleFaces => "cross_check_rgb_multiple_faces",
        MatchReason::CrossCheckIrNoFace => "cross_check_ir_no_face",
        MatchReason::CrossCheckIrMultipleFaces => "cross_check_ir_multiple_faces",
        MatchReason::CrossCheckPositionMismatch => "cross_check_position_mismatch",
        MatchReason::Internal => "internal",
    }
}

pub fn match_reason_human(reason: MatchReason) -> &'static str {
    match reason {
        MatchReason::Matched => "matched",
        MatchReason::TemplateMismatch => "best template score is below threshold",
        MatchReason::NotEnrolled => "no enrolled template",
        MatchReason::NoFace => "no face detected",
        MatchReason::MultipleFaces => "multiple faces detected",
        MatchReason::CrossCheckRequired => "RGB+IR cross-check is required",
        MatchReason::CrossCheckTimeSkew => "RGB and IR frames were not synchronized",
        MatchReason::CrossCheckRgbNoFace => "no face detected on RGB frame",
        MatchReason::CrossCheckRgbMultipleFaces => "multiple faces detected on RGB frame",
        MatchReason::CrossCheckIrNoFace => "no face detected on IR frame",
        MatchReason::CrossCheckIrMultipleFaces => "multiple faces detected on IR frame",
        MatchReason::CrossCheckPositionMismatch => "RGB/IR face positions do not align",
        MatchReason::Internal => "internal broker error",
    }
}

pub fn match_reason_is_retryable_capture(reason: MatchReason) -> bool {
    matches!(
        reason,
        MatchReason::CrossCheckTimeSkew
            | MatchReason::CrossCheckRgbNoFace
            | MatchReason::CrossCheckRgbMultipleFaces
            | MatchReason::CrossCheckIrNoFace
            | MatchReason::CrossCheckIrMultipleFaces
            | MatchReason::CrossCheckPositionMismatch
    )
}

/// True when the operator has configured a usable RGB+IR cross-check: the
/// dedicated `[camera.ir]` section is present *and* `[camera.cross_check]` is
/// enabled.
pub fn cross_check_active(config: &Config) -> bool {
    config.camera.cross_check.enabled && config.camera.ir.is_some()
}

/// Open the RGB camera using the top-level `[camera]` settings.
pub fn open_rgb_camera(config: &Config) -> std::result::Result<V4lCamera, FaceRsError> {
    let mut cam = V4lCamera::open(
        &config.camera.device,
        config.camera.width,
        config.camera.height,
        config.camera.fps,
        config.camera.timeout_ms,
    )?;
    cam.warmup(config.camera.warmup_frames);
    Ok(cam)
}

/// Open the IR camera using `[camera.ir]` overrides where set, otherwise IR-
/// friendly defaults (longer timeout, more warmup frames, can stream at the
/// sensor's native resolution).
pub fn open_ir_camera(config: &Config) -> std::result::Result<V4lCamera, FaceRsError> {
    let ir = config.camera.ir.as_ref().ok_or_else(|| {
        FaceRsError::Config("camera.ir is not configured but IR capture was requested".to_owned())
    })?;
    let mut cam = V4lCamera::open(
        &ir.device,
        ir.effective_width(config.camera.width),
        ir.effective_height(config.camera.height),
        ir.effective_fps(config.camera.fps),
        ir.effective_timeout_ms(config.camera.timeout_ms),
    )?;
    cam.warmup(ir.effective_warmup_frames(config.camera.warmup_frames));
    Ok(cam)
}

/// Capture one RGB and one IR frame in parallel scoped threads so the
/// captured_at_ms timestamps are as close together as the V4L2 layers allow.
/// Each side reports its own error; this is what feeds the broker's sync
/// window check.
pub fn capture_rgb_ir_pair(
    rgb: &mut V4lCamera,
    ir: &mut V4lCamera,
) -> (
    std::result::Result<Frame, FaceRsError>,
    std::result::Result<Frame, FaceRsError>,
) {
    std::thread::scope(|s| {
        let rgb_handle = s.spawn(|| rgb.capture_frame());
        let ir_handle = s.spawn(|| ir.capture_frame());
        let rgb_result = rgb_handle.join().unwrap_or_else(|_| {
            Err(FaceRsError::Camera(
                "RGB capture thread panicked".to_owned(),
            ))
        });
        let ir_result = ir_handle
            .join()
            .unwrap_or_else(|_| Err(FaceRsError::Camera("IR capture thread panicked".to_owned())));
        (rgb_result, ir_result)
    })
}

pub fn summary_allows(template: &EnrolledTemplateSummary, auth_scope: AuthScope) -> bool {
    matches!(
        (template.scope, auth_scope),
        (facegate_ipc::TemplateScope::Both, _)
            | (facegate_ipc::TemplateScope::Sudo, AuthScope::Sudo)
            | (facegate_ipc::TemplateScope::Session, AuthScope::Session)
    )
}

pub fn summary_scope_label(template: &EnrolledTemplateSummary) -> &'static str {
    match template.scope {
        facegate_ipc::TemplateScope::Sudo => "sudo",
        facegate_ipc::TemplateScope::Session => "session",
        facegate_ipc::TemplateScope::Both => "both",
    }
}

pub fn summary_to_enrolled(template: EnrolledTemplateSummary) -> EnrolledTemplate {
    EnrolledTemplate {
        id: template.id,
        label: template.label,
        created_at: template.created_at,
        scope: core_template_scope(template.scope),
        embedding: Vec::new(),
    }
}

fn request(request: Request) -> Result<Response> {
    let response = request_raw(request)?;
    match response {
        Response::Error(error) => match error.code {
            ErrorCode::NotEnrolled => bail!("user has no enrolled templates"),
            _ => bail!("broker error {:?}: {}", error.code, error.message),
        },
        response => Ok(response),
    }
}

fn request_raw(request: Request) -> Result<Response> {
    let response = send_request(DEFAULT_SOCKET_PATH, RequestEnvelope::new(request))?;
    Ok(response.response)
}

fn ipc_auth_scope(value: AuthScope) -> facegate_ipc::AuthScope {
    match value {
        AuthScope::Sudo => facegate_ipc::AuthScope::Sudo,
        AuthScope::Session => facegate_ipc::AuthScope::Session,
    }
}

fn ipc_template_scope(value: TemplateScope) -> facegate_ipc::TemplateScope {
    match value {
        TemplateScope::Sudo => facegate_ipc::TemplateScope::Sudo,
        TemplateScope::Session => facegate_ipc::TemplateScope::Session,
        TemplateScope::Both => facegate_ipc::TemplateScope::Both,
    }
}

fn core_template_scope(value: facegate_ipc::TemplateScope) -> TemplateScope {
    match value {
        facegate_ipc::TemplateScope::Sudo => TemplateScope::Sudo,
        facegate_ipc::TemplateScope::Session => TemplateScope::Session,
        facegate_ipc::TemplateScope::Both => TemplateScope::Both,
    }
}
