use std::ffi::CString;
use std::fs;
use std::io::{BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use facegate_core::config::{Config, DEFAULT_CONFIG_PATH};
use facegate_core::error::FaceRsError;
use facegate_core::matching::cosine_similarity;
use facegate_core::storage::{AuthScope, TemplateScope, TemplateStore};
use facegate_ipc::{
    encode_response, AuditEvent, AuditOutcome, AuditReason, BrokerInfo, EnrolledTemplateSummary,
    ErrorCode, MatchResult, Request, RequestEnvelope, Response, ResponseEnvelope, PROTOCOL_VERSION,
};
use std::os::unix::fs::FileTypeExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task;
use zeroize::Zeroize;

const DEFAULT_SOCKET_PATH: &str = "/run/facegate/broker.sock";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const RATE_LIMIT_MAX_MATCHES: u32 = 60;
const FAILURE_WINDOW: Duration = Duration::from_secs(300);

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let socket_path = socket_path();
    let config_path = config_path();
    let state = BrokerState::from_config(Config::load(&config_path).unwrap_or_else(|e| {
        tracing::warn!("cannot load config, using defaults: {e}");
        Config::default()
    }));
    run(socket_path, state).await
}

fn init_logging() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn socket_path() -> PathBuf {
    std::env::var_os("FACEGATE_BROKER_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET_PATH))
}

fn config_path() -> PathBuf {
    std::env::var_os("FACEGATE_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH))
}

async fn run(socket_path: PathBuf, state: BrokerState) -> Result<()> {
    prepare_socket_path(&socket_path)?;
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("cannot bind {}", socket_path.display()))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666))
        .with_context(|| format!("cannot set permissions on {}", socket_path.display()))?;

    tracing::info!(socket = %socket_path.display(), "facegate broker listening");

    loop {
        let (stream, _addr) = listener.accept().await.context("accept failed")?;
        let peer = PeerCredentials::from_stream(&stream).ok();
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, state, peer).await {
                tracing::warn!("broker client error: {e}");
            }
        });
    }
}

fn prepare_socket_path(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("cannot create {}", parent.display()))?;

    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            std::fs::remove_file(path)
                .with_context(|| format!("cannot remove stale socket {}", path.display()))?;
        }
        Ok(_) => {
            anyhow::bail!("{} exists and is not a Unix socket", path.display());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("cannot inspect {}", path.display())),
    }
    Ok(())
}

async fn handle_client(
    stream: UnixStream,
    state: BrokerState,
    peer: Option<PeerCredentials>,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    let n = reader
        .read_until(b'\n', &mut line)
        .await
        .context("cannot read request")?;
    if n == 0 {
        return Ok(());
    }
    if line.len() > MAX_REQUEST_BYTES {
        return write_response(
            reader.get_mut(),
            ResponseEnvelope::error(ErrorCode::BadRequest, "request too large"),
        )
        .await;
    }

    let response = match serde_json::from_slice::<RequestEnvelope>(&line) {
        Ok(envelope) => {
            let state = state.clone();
            task::spawn_blocking(move || state.dispatch(envelope, peer))
                .await
                .context("broker task failed")?
        }
        Err(_) => ResponseEnvelope::error(ErrorCode::BadRequest, "invalid request JSON"),
    };
    write_response(reader.get_mut(), response).await
}

#[derive(Debug, Clone)]
struct BrokerState {
    storage_base_dir: PathBuf,
    audit_path: PathBuf,
    threshold: f32,
    cooldown_after_failures: u32,
    cooldown_duration: Duration,
    abuse: Arc<Mutex<AbuseState>>,
}

impl BrokerState {
    fn from_config(config: Config) -> Self {
        let audit_path = config
            .storage
            .base_dir
            .parent()
            .map(|path| path.join("audit.log"))
            .unwrap_or_else(|| PathBuf::from("/var/lib/facegate/audit.log"));
        Self {
            storage_base_dir: config.storage.base_dir,
            audit_path,
            threshold: config.recognition.threshold,
            cooldown_after_failures: config.security.cooldown_after_failures,
            cooldown_duration: Duration::from_secs(config.security.cooldown_seconds),
            abuse: Arc::new(Mutex::new(AbuseState::default())),
        }
    }

    fn dispatch(
        &self,
        envelope: RequestEnvelope,
        peer: Option<PeerCredentials>,
    ) -> ResponseEnvelope {
        if envelope.version != PROTOCOL_VERSION {
            return ResponseEnvelope::error(
                ErrorCode::VersionMismatch,
                format!(
                    "unsupported protocol version {}; expected {}",
                    envelope.version, PROTOCOL_VERSION
                ),
            );
        }

        match envelope.request {
            Request::Health => ResponseEnvelope::ok(Response::Health {
                info: BrokerInfo {
                    protocol_version: PROTOCOL_VERSION,
                    broker_version: env!("CARGO_PKG_VERSION").to_owned(),
                },
            }),
            Request::AuditRecent { username, limit } => self.audit_recent(peer, username, limit),
            Request::Match {
                username,
                auth_scope,
                mut probe_embedding,
            } => {
                let response = self.match_embedding(
                    peer,
                    &username,
                    core_auth_scope(auth_scope),
                    &probe_embedding,
                );
                probe_embedding.zeroize();
                response
            }
            Request::MatchFrame { .. } => ResponseEnvelope::error(
                ErrorCode::Unsupported,
                "frame-based matching is not implemented yet",
            ),
            Request::Enroll {
                username,
                label,
                scope,
                mut embedding,
            } => {
                let response = self.enroll(
                    peer,
                    &username,
                    &label,
                    core_template_scope(scope),
                    embedding.clone(),
                );
                embedding.zeroize();
                response
            }
            Request::List { username } => self.list(peer, &username),
            Request::Remove {
                username,
                template_id,
            } => self.remove(peer, &username, template_id),
        }
    }

    fn store(&self) -> TemplateStore {
        TemplateStore::new(&self.storage_base_dir)
    }

    fn match_embedding(
        &self,
        peer: Option<PeerCredentials>,
        username: &str,
        auth_scope: AuthScope,
        probe_embedding: &[f32],
    ) -> ResponseEnvelope {
        if !authorized_for_match(peer, username, auth_scope) {
            self.write_audit(
                username,
                auth_scope,
                AuditOutcome::Failure,
                AuditReason::Unauthorized,
            );
            return unauthorized();
        }
        let Some(peer) = peer else {
            self.write_audit(
                username,
                auth_scope,
                AuditOutcome::Failure,
                AuditReason::Unauthorized,
            );
            return unauthorized();
        };
        match self.check_match_allowed(peer, username) {
            Ok(()) => {}
            Err((error, reason)) => {
                self.write_audit(username, auth_scope, AuditOutcome::Failure, reason);
                return error;
            }
        }

        let store = self.store();
        let mut templates = match store.load(username) {
            Ok(store) => store.templates,
            Err(e) => {
                self.record_match_failure(username);
                self.write_audit(
                    username,
                    auth_scope,
                    AuditOutcome::Failure,
                    AuditReason::Internal,
                );
                return storage_error(e);
            }
        };

        let mut best: Option<(u32, f32)> = None;
        for template in templates
            .iter()
            .filter(|template| template.scope.allows(auth_scope))
        {
            let score = cosine_similarity(probe_embedding, &template.embedding);
            if best
                .map(|(_, best_score)| score > best_score)
                .unwrap_or(true)
            {
                best = Some((template.id, score));
            }
        }

        let Some((template_id, score)) = best else {
            self.zeroize_templates(&mut templates);
            self.record_match_failure(username);
            self.write_audit(
                username,
                auth_scope,
                AuditOutcome::Failure,
                AuditReason::NotEnrolled,
            );
            return ResponseEnvelope::error(
                ErrorCode::NotEnrolled,
                "user has no enrolled templates",
            );
        };
        let matched = score >= self.threshold;
        self.zeroize_templates(&mut templates);
        if matched {
            self.record_match_success(username);
            self.write_audit(
                username,
                auth_scope,
                AuditOutcome::Success,
                AuditReason::Matched,
            );
        } else {
            self.record_match_failure(username);
            self.write_audit(
                username,
                auth_scope,
                AuditOutcome::Failure,
                AuditReason::Mismatch,
            );
        }
        ResponseEnvelope::ok(Response::Match {
            result: MatchResult {
                matched,
                score: Some(score),
                template_id: matched.then_some(template_id),
            },
        })
    }

    fn enroll(
        &self,
        peer: Option<PeerCredentials>,
        username: &str,
        label: &str,
        scope: TemplateScope,
        embedding: Vec<f32>,
    ) -> ResponseEnvelope {
        if !peer.map(|p| p.uid == 0).unwrap_or(false) {
            return unauthorized();
        }

        match self.store().add_template(username, label, scope, embedding) {
            Ok(mut template) => {
                let summary = EnrolledTemplateSummary {
                    id: template.id,
                    label: template.label.clone(),
                    created_at: template.created_at.clone(),
                    scope: ipc_template_scope(template.scope),
                };
                template.embedding.zeroize();
                ResponseEnvelope::ok(Response::Enrolled { template: summary })
            }
            Err(e) => storage_error(e),
        }
    }

    fn list(&self, peer: Option<PeerCredentials>, username: &str) -> ResponseEnvelope {
        if !authorized_for_user_or_root(peer, username) {
            return unauthorized();
        }

        match self.store().load(username) {
            Ok(mut store) => {
                let templates = store
                    .templates
                    .iter_mut()
                    .map(|template| EnrolledTemplateSummary {
                        id: template.id,
                        label: template.label.clone(),
                        created_at: template.created_at.clone(),
                        scope: ipc_template_scope(template.scope),
                    })
                    .collect();
                self.zeroize_templates(&mut store.templates);
                ResponseEnvelope::ok(Response::List { templates })
            }
            Err(e) => storage_error(e),
        }
    }

    fn remove(
        &self,
        peer: Option<PeerCredentials>,
        username: &str,
        template_id: u32,
    ) -> ResponseEnvelope {
        if !peer.map(|p| p.uid == 0).unwrap_or(false) {
            return unauthorized();
        }

        match self.store().remove_template(username, template_id) {
            Ok(()) => ResponseEnvelope::ok(Response::Removed),
            Err(e) => storage_error(e),
        }
    }

    fn zeroize_templates(&self, templates: &mut [facegate_core::storage::EnrolledTemplate]) {
        for template in templates {
            template.embedding.zeroize();
        }
    }

    fn check_match_allowed(
        &self,
        peer: PeerCredentials,
        username: &str,
    ) -> std::result::Result<(), (ResponseEnvelope, AuditReason)> {
        let mut abuse = self.abuse.lock().unwrap_or_else(|e| e.into_inner());
        abuse.check_match_allowed(peer.uid, username)
    }

    fn record_match_success(&self, username: &str) {
        let mut abuse = self.abuse.lock().unwrap_or_else(|e| e.into_inner());
        abuse.record_success(username);
    }

    fn record_match_failure(&self, username: &str) {
        let mut abuse = self.abuse.lock().unwrap_or_else(|e| e.into_inner());
        abuse.record_failure(
            username,
            self.cooldown_after_failures,
            self.cooldown_duration,
        );
    }

    fn audit_recent(
        &self,
        peer: Option<PeerCredentials>,
        username: Option<String>,
        limit: u32,
    ) -> ResponseEnvelope {
        let Some(peer) = peer else {
            return unauthorized();
        };
        let filter_user = match username {
            Some(username)
                if peer.uid == 0
                    || uid_for_username(&username).ok().flatten() == Some(peer.uid) =>
            {
                Some(username)
            }
            Some(_) => return unauthorized(),
            None if peer.uid == 0 => None,
            None => return unauthorized(),
        };

        let events = self
            .read_audit_events(filter_user.as_deref(), limit.clamp(1, 50) as usize)
            .unwrap_or_default();
        ResponseEnvelope::ok(Response::Audit { events })
    }

    fn write_audit(
        &self,
        username: &str,
        auth_scope: AuthScope,
        outcome: AuditOutcome,
        reason: AuditReason,
    ) {
        let event = AuditEvent {
            timestamp_unix: unix_now(),
            username: username.to_owned(),
            auth_scope: ipc_auth_scope(auth_scope),
            outcome,
            reason,
        };
        let Ok(mut line) = serde_json::to_string(&event) else {
            return;
        };
        line.push('\n');
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn read_audit_events(
        &self,
        username: Option<&str>,
        limit: usize,
    ) -> std::io::Result<Vec<AuditEvent>> {
        let file = fs::File::open(&self.audit_path)?;
        let reader = std::io::BufReader::new(file);
        let mut events = reader
            .lines()
            .map_while(std::result::Result::ok)
            .filter_map(|line| serde_json::from_str::<AuditEvent>(&line).ok())
            .filter(|event| username.map(|u| event.username == u).unwrap_or(true))
            .collect::<Vec<_>>();
        let keep_from = events.len().saturating_sub(limit);
        Ok(events.split_off(keep_from))
    }
}

#[derive(Debug, Default)]
struct AbuseState {
    peer_limits: std::collections::HashMap<u32, WindowCounter>,
    user_failures: std::collections::HashMap<String, FailureCounter>,
}

impl AbuseState {
    fn check_match_allowed(
        &mut self,
        peer_uid: u32,
        username: &str,
    ) -> std::result::Result<(), (ResponseEnvelope, AuditReason)> {
        let now = Instant::now();
        let peer = self
            .peer_limits
            .entry(peer_uid)
            .or_insert_with(|| WindowCounter {
                window_started: now,
                count: 0,
            });
        if now.duration_since(peer.window_started) > RATE_LIMIT_WINDOW {
            peer.window_started = now;
            peer.count = 0;
        }
        if peer.count >= RATE_LIMIT_MAX_MATCHES {
            return Err((
                ResponseEnvelope::error(ErrorCode::RateLimited, "too many match requests"),
                AuditReason::RateLimited,
            ));
        }
        peer.count += 1;

        let failure = self
            .user_failures
            .entry(username.to_owned())
            .or_insert_with(|| FailureCounter {
                window_started: now,
                failures: 0,
                locked_until: None,
            });
        if let Some(locked_until) = failure.locked_until {
            if now < locked_until {
                return Err((
                    ResponseEnvelope::error(ErrorCode::LockedOut, "too many failed match attempts"),
                    AuditReason::LockedOut,
                ));
            }
            failure.locked_until = None;
            failure.failures = 0;
            failure.window_started = now;
        }

        Ok(())
    }

    fn record_success(&mut self, username: &str) {
        self.user_failures.remove(username);
    }

    fn record_failure(&mut self, username: &str, lock_threshold: u32, lock_duration: Duration) {
        let now = Instant::now();
        let failure = self
            .user_failures
            .entry(username.to_owned())
            .or_insert_with(|| FailureCounter {
                window_started: now,
                failures: 0,
                locked_until: None,
            });
        if now.duration_since(failure.window_started) > FAILURE_WINDOW {
            failure.window_started = now;
            failure.failures = 0;
            failure.locked_until = None;
        }
        failure.failures = failure.failures.saturating_add(1);
        if failure.failures >= lock_threshold {
            failure.locked_until = Some(now + lock_duration);
        }
    }
}

#[derive(Debug)]
struct WindowCounter {
    window_started: Instant,
    count: u32,
}

#[derive(Debug)]
struct FailureCounter {
    window_started: Instant,
    failures: u32,
    locked_until: Option<Instant>,
}

async fn write_response(stream: &mut UnixStream, response: ResponseEnvelope) -> Result<()> {
    let encoded = encode_response(&response).context("cannot encode response")?;
    stream
        .write_all(&encoded)
        .await
        .context("cannot write response")?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct PeerCredentials {
    uid: u32,
}

impl PeerCredentials {
    fn from_stream(stream: &UnixStream) -> std::io::Result<Self> {
        let cred = stream.peer_cred()?;
        Ok(Self { uid: cred.uid() })
    }
}

fn authorized_for_match(
    peer: Option<PeerCredentials>,
    username: &str,
    auth_scope: AuthScope,
) -> bool {
    let Some(peer) = peer else {
        return false;
    };
    if peer.uid == 0 {
        return true;
    }
    auth_scope == AuthScope::Session && uid_for_username(username).ok().flatten() == Some(peer.uid)
}

fn authorized_for_user_or_root(peer: Option<PeerCredentials>, username: &str) -> bool {
    let Some(peer) = peer else {
        return false;
    };
    peer.uid == 0 || uid_for_username(username).ok().flatten() == Some(peer.uid)
}

fn uid_for_username(username: &str) -> Result<Option<u32>> {
    let c_name =
        CString::new(username).with_context(|| format!("invalid username '{username}'"))?;
    let mut buf = vec![0i8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    let rc = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::from_raw_os_error(rc)).context("getpwnam_r failed");
    }
    if result.is_null() {
        Ok(None)
    } else {
        Ok(Some(pwd.pw_uid))
    }
}

fn unauthorized() -> ResponseEnvelope {
    ResponseEnvelope::error(ErrorCode::Unauthorized, "unauthorized broker request")
}

fn storage_error(error: FaceRsError) -> ResponseEnvelope {
    match error {
        FaceRsError::NotEnrolled => {
            ResponseEnvelope::error(ErrorCode::NotEnrolled, "user has no enrolled templates")
        }
        other => ResponseEnvelope::error(ErrorCode::Internal, other.to_string()),
    }
}

fn core_auth_scope(value: facegate_ipc::AuthScope) -> AuthScope {
    match value {
        facegate_ipc::AuthScope::Sudo => AuthScope::Sudo,
        facegate_ipc::AuthScope::Session => AuthScope::Session,
    }
}

fn ipc_auth_scope(value: AuthScope) -> facegate_ipc::AuthScope {
    match value {
        AuthScope::Sudo => facegate_ipc::AuthScope::Sudo,
        AuthScope::Session => facegate_ipc::AuthScope::Session,
    }
}

fn core_template_scope(value: facegate_ipc::TemplateScope) -> TemplateScope {
    match value {
        facegate_ipc::TemplateScope::Sudo => TemplateScope::Sudo,
        facegate_ipc::TemplateScope::Session => TemplateScope::Session,
        facegate_ipc::TemplateScope::Both => TemplateScope::Both,
    }
}

fn ipc_template_scope(value: TemplateScope) -> facegate_ipc::TemplateScope {
    match value {
        TemplateScope::Sudo => facegate_ipc::TemplateScope::Sudo,
        TemplateScope::Session => facegate_ipc::TemplateScope::Session,
        TemplateScope::Both => facegate_ipc::TemplateScope::Both,
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use facegate_ipc::{AuthScope as IpcAuthScope, TemplateScope as IpcTemplateScope};

    fn unit_vec(v: &[f32]) -> Vec<f32> {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / norm).collect()
    }

    fn test_state() -> (tempfile::TempDir, BrokerState) {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = BrokerState {
            storage_base_dir: dir.path().to_owned(),
            audit_path: dir.path().join("audit.log"),
            threshold: 0.55,
            cooldown_after_failures: 10,
            cooldown_duration: Duration::from_secs(60),
            abuse: Arc::new(Mutex::new(AbuseState::default())),
        };
        (dir, state)
    }

    #[test]
    fn health_response_uses_current_protocol() {
        let (_dir, state) = test_state();
        let response = state.dispatch(RequestEnvelope::new(Request::Health), None);
        let Response::Health { info } = response.response else {
            panic!("expected health response");
        };
        assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn rejects_protocol_mismatch() {
        let (_dir, state) = test_state();
        let response = state.dispatch(
            RequestEnvelope {
                version: PROTOCOL_VERSION + 1,
                request: Request::Health,
            },
            None,
        );
        let Response::Error(error) = response.response else {
            panic!("expected error response");
        };
        assert_eq!(error.code, ErrorCode::VersionMismatch);
    }

    #[test]
    fn list_does_not_expose_embeddings() {
        let (_dir, state) = test_state();
        let peer = Some(PeerCredentials { uid: 0 });

        let enrolled = state.dispatch(
            RequestEnvelope::new(Request::Enroll {
                username: "alice".to_owned(),
                label: "front".to_owned(),
                scope: IpcTemplateScope::Both,
                embedding: unit_vec(&[1.0, 0.0]),
            }),
            peer,
        );
        assert!(matches!(enrolled.response, Response::Enrolled { .. }));

        let listed = state.dispatch(
            RequestEnvelope::new(Request::List {
                username: "alice".to_owned(),
            }),
            peer,
        );
        let Response::List { templates } = listed.response else {
            panic!("expected list response");
        };
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].label, "front");

        let json = serde_json::to_string(&ResponseEnvelope::ok(Response::List { templates }))
            .expect("serialize");
        assert!(!json.contains("embedding"));
    }

    #[test]
    fn match_returns_decision_without_vector() {
        let (_dir, state) = test_state();
        let peer = Some(PeerCredentials { uid: 0 });
        let embedding = unit_vec(&[1.0, 0.0]);

        state.dispatch(
            RequestEnvelope::new(Request::Enroll {
                username: "alice".to_owned(),
                label: "front".to_owned(),
                scope: IpcTemplateScope::Session,
                embedding: embedding.clone(),
            }),
            peer,
        );

        let matched = state.dispatch(
            RequestEnvelope::new(Request::Match {
                username: "alice".to_owned(),
                auth_scope: IpcAuthScope::Session,
                probe_embedding: embedding,
            }),
            peer,
        );
        let Response::Match { result } = matched.response else {
            panic!("expected match response");
        };

        assert!(result.matched);
        assert_eq!(result.template_id, Some(0));
    }

    #[test]
    fn match_writes_audit_event() {
        let (_dir, state) = test_state();
        let peer = Some(PeerCredentials { uid: 0 });
        let embedding = unit_vec(&[1.0, 0.0]);

        state.dispatch(
            RequestEnvelope::new(Request::Enroll {
                username: "alice".to_owned(),
                label: "front".to_owned(),
                scope: IpcTemplateScope::Session,
                embedding: embedding.clone(),
            }),
            peer,
        );
        state.dispatch(
            RequestEnvelope::new(Request::Match {
                username: "alice".to_owned(),
                auth_scope: IpcAuthScope::Session,
                probe_embedding: embedding,
            }),
            peer,
        );

        let audit = state.dispatch(
            RequestEnvelope::new(Request::AuditRecent {
                username: Some("alice".to_owned()),
                limit: 5,
            }),
            peer,
        );
        let Response::Audit { events } = audit.response else {
            panic!("expected audit response");
        };
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].username, "alice");
        assert_eq!(events[0].outcome, AuditOutcome::Success);
        assert_eq!(events[0].reason, AuditReason::Matched);
    }

    #[test]
    fn rate_limits_match_requests_per_peer() {
        let (_dir, state) = test_state();
        let peer = Some(PeerCredentials { uid: 0 });

        for _ in 0..RATE_LIMIT_MAX_MATCHES {
            let response = state.dispatch(
                RequestEnvelope::new(Request::Match {
                    username: "alice".to_owned(),
                    auth_scope: IpcAuthScope::Session,
                    probe_embedding: vec![1.0],
                }),
                peer,
            );
            assert!(matches!(response.response, Response::Error(_)));
        }

        let response = state.dispatch(
            RequestEnvelope::new(Request::Match {
                username: "alice".to_owned(),
                auth_scope: IpcAuthScope::Session,
                probe_embedding: vec![1.0],
            }),
            peer,
        );
        let Response::Error(error) = response.response else {
            panic!("expected rate limit error");
        };
        assert_eq!(error.code, ErrorCode::RateLimited);
    }

    #[test]
    fn locks_out_after_repeated_failed_matches() {
        let (_dir, state) = test_state();
        let peer = Some(PeerCredentials { uid: 0 });

        for _ in 0..state.cooldown_after_failures {
            let response = state.dispatch(
                RequestEnvelope::new(Request::Match {
                    username: "alice".to_owned(),
                    auth_scope: IpcAuthScope::Session,
                    probe_embedding: vec![1.0],
                }),
                peer,
            );
            assert!(matches!(response.response, Response::Error(_)));
        }

        let response = state.dispatch(
            RequestEnvelope::new(Request::Match {
                username: "alice".to_owned(),
                auth_scope: IpcAuthScope::Session,
                probe_embedding: vec![1.0],
            }),
            peer,
        );
        let Response::Error(error) = response.response else {
            panic!("expected lockout error");
        };
        assert_eq!(error.code, ErrorCode::LockedOut);
    }

    #[test]
    fn non_root_cannot_enroll() {
        let (_dir, state) = test_state();
        let response = state.dispatch(
            RequestEnvelope::new(Request::Enroll {
                username: "alice".to_owned(),
                label: "front".to_owned(),
                scope: IpcTemplateScope::Both,
                embedding: vec![1.0],
            }),
            Some(PeerCredentials { uid: 1000 }),
        );

        let Response::Error(error) = response.response else {
            panic!("expected error response");
        };
        assert_eq!(error.code, ErrorCode::Unauthorized);
    }
}
