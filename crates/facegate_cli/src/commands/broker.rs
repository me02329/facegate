use anyhow::{bail, Result};
use facegate_core::camera::Frame;
use facegate_core::storage::{AuthScope, EnrolledTemplate, TemplateScope};
use facegate_ipc::{
    send_request, AuditEvent, BrokerError, EnrolledTemplateSummary, ErrorCode, FrameFormat,
    FrameProbe, MatchResult, Request, RequestEnvelope, Response, DEFAULT_SOCKET_PATH,
};

/// Wrap a freshly captured `Frame` in a `FrameProbe`, tagging it with the
/// current wall-clock time so the broker can apply the RGB+IR sync window.
/// `Frame::data` is always RGB24 (the camera layer converts YUYV / MJPEG /
/// GREY → RGB before returning), so the format is always `Rgb8`.
pub fn frame_probe(frame: Frame) -> FrameProbe {
    FrameProbe {
        format: FrameFormat::Rgb8,
        width: frame.width,
        height: frame.height,
        captured_at_ms: now_ms(),
        bytes: frame.data,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
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

pub fn match_embedding_optional(
    username: &str,
    auth_scope: AuthScope,
    probe_embedding: Vec<f32>,
) -> Result<Option<MatchResult>> {
    match request_raw(match_request(username, auth_scope, probe_embedding))? {
        Response::Match { result } => Ok(Some(result)),
        Response::Error(error) if error.code == ErrorCode::NotEnrolled => Ok(None),
        Response::Error(error) => bail!("broker error {:?}: {}", error.code, error.message),
        other => bail!("unexpected broker response: {other:?}"),
    }
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
