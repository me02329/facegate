use std::ffi::CString;
use std::fs;
use std::io::{BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use facegate_core::camera::Frame;
use facegate_core::config::{CameraConfig, Config, ModelsConfig, DEFAULT_CONFIG_PATH};
use facegate_core::detection::{Detection, ScrfdDetector};
use facegate_core::embedding::ArcFaceEmbedder;
use facegate_core::error::FaceRsError;
use facegate_core::matching::cosine_similarity;
use facegate_core::storage::{AuthScope, TemplateScope, TemplateStore};
use facegate_ipc::{
    encode_response, AuditEvent, AuditOutcome, AuditReason, BrokerInfo, EnrolledTemplateSummary,
    ErrorCode, FrameFormat, FrameProbe, MatchResult, Request, RequestEnvelope, Response,
    ResponseEnvelope, MAX_REQUEST_BYTES, PROTOCOL_VERSION,
};
use std::os::unix::fs::FileTypeExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task;
use zeroize::Zeroize;

const DEFAULT_SOCKET_PATH: &str = "/run/facegate/broker.sock";
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const RATE_LIMIT_MAX_MATCHES: u32 = 60;
const FAILURE_WINDOW: Duration = Duration::from_secs(300);
/// Reject frames whose declared geometry exceeds this bound before any
/// allocation. Above 4K, broker-side inference is not realistic anyway.
const MAX_FRAME_PIXELS: u32 = 4096 * 4096;

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

#[derive(Clone)]
struct BrokerState {
    storage_base_dir: PathBuf,
    audit_path: PathBuf,
    threshold: f32,
    min_face_size: u32,
    cooldown_after_failures: u32,
    cooldown_duration: Duration,
    cross_check: CrossCheckPolicy,
    abuse: Arc<Mutex<AbuseState>>,
    models: ModelsConfig,
    /// SCRFD + ArcFace, lazily loaded on first MatchFrame request. Guarded by
    /// a Mutex because ort sessions hold non-Send state once mid-inference and
    /// we serialise inference anyway — there is no benefit to parallelism here.
    inference: Arc<Mutex<Option<InferenceState>>>,
}

struct InferenceState {
    detector: ScrfdDetector,
    embedder: ArcFaceEmbedder,
}

struct InferenceProbe {
    detection: Detection,
    embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
struct CrossCheckPolicy {
    required: bool,
    max_time_skew_ms: u64,
    max_position_offset_px: f32,
    min_identity_similarity: f32,
    homography: [f32; 9],
}

impl CrossCheckPolicy {
    fn from_config(config: &CameraConfig) -> Self {
        Self {
            required: config.cross_check.enabled && config.ir_device.is_some(),
            max_time_skew_ms: config.cross_check.max_time_skew_ms,
            max_position_offset_px: config.cross_check.max_position_offset_px,
            min_identity_similarity: config.cross_check.min_identity_similarity,
            homography: config.cross_check.homography,
        }
    }
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
            min_face_size: config.recognition.min_face_size,
            cooldown_after_failures: config.security.cooldown_after_failures,
            cooldown_duration: Duration::from_secs(config.security.cooldown_seconds),
            cross_check: CrossCheckPolicy::from_config(&config.camera),
            abuse: Arc::new(Mutex::new(AbuseState::default())),
            models: config.models,
            inference: Arc::new(Mutex::new(None)),
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
                // Admin-only since v2: non-root callers cannot bypass live capture by
                // submitting a precomputed embedding. Auth paths must use MatchFrame.
                let response = if peer.map(|p| p.uid == 0).unwrap_or(false) {
                    self.match_embedding(
                        peer,
                        &username,
                        core_auth_scope(auth_scope),
                        &probe_embedding,
                    )
                } else {
                    unauthorized()
                };
                probe_embedding.zeroize();
                response
            }
            Request::MatchFrame {
                username,
                auth_scope,
                mut frame,
            } => {
                let response =
                    self.match_frame(peer, &username, core_auth_scope(auth_scope), &frame);
                frame.bytes.zeroize();
                response
            }
            Request::MatchFramePair {
                username,
                auth_scope,
                mut rgb_frame,
                mut ir_frame,
            } => {
                let response = self.match_frame_pair(
                    peer,
                    &username,
                    core_auth_scope(auth_scope),
                    &rgb_frame,
                    &ir_frame,
                );
                rgb_frame.bytes.zeroize();
                ir_frame.bytes.zeroize();
                response
            }
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

        self.compare_embedding(username, auth_scope, probe_embedding)
    }

    /// Compare a probe embedding against the stored templates for `username`.
    /// Assumes peer authorization and rate-limit checks have already been
    /// performed; writes the final outcome to the audit log and returns the
    /// response envelope.
    fn compare_embedding(
        &self,
        username: &str,
        auth_scope: AuthScope,
        probe_embedding: &[f32],
    ) -> ResponseEnvelope {
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

    fn match_frame(
        &self,
        peer: Option<PeerCredentials>,
        username: &str,
        auth_scope: AuthScope,
        probe: &FrameProbe,
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

        if self.cross_check.required {
            return self.cross_check_mismatch(username, auth_scope);
        }

        let mut frame = match frame_from_probe(probe) {
            Ok(frame) => frame,
            Err(err) => {
                self.record_match_failure(username);
                self.write_audit(
                    username,
                    auth_scope,
                    AuditOutcome::Failure,
                    AuditReason::Internal,
                );
                return err;
            }
        };

        let mut probe_embedding = match self.run_inference(&frame) {
            Ok(Some(embedding)) => embedding,
            Ok(None) => {
                frame.data.zeroize();
                // No face detected — treated as a mismatch so rate limit /
                // lockout counters apply, but reported with score=None so the
                // client can distinguish a no-face frame from a low-score
                // match.
                self.record_match_failure(username);
                self.write_audit(
                    username,
                    auth_scope,
                    AuditOutcome::Failure,
                    AuditReason::Mismatch,
                );
                return ResponseEnvelope::ok(Response::Match {
                    result: MatchResult {
                        matched: false,
                        score: None,
                        template_id: None,
                    },
                });
            }
            Err(err) => {
                frame.data.zeroize();
                self.record_match_failure(username);
                self.write_audit(
                    username,
                    auth_scope,
                    AuditOutcome::Failure,
                    AuditReason::Internal,
                );
                return err;
            }
        };
        frame.data.zeroize();

        let response = self.compare_embedding(username, auth_scope, &probe_embedding);
        probe_embedding.zeroize();
        response
    }

    fn match_frame_pair(
        &self,
        peer: Option<PeerCredentials>,
        username: &str,
        auth_scope: AuthScope,
        rgb_probe: &FrameProbe,
        ir_probe: &FrameProbe,
    ) -> ResponseEnvelope {
        if !self.cross_check.required {
            return self.match_frame(peer, username, auth_scope, rgb_probe);
        }
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

        if !frames_are_synchronized(
            rgb_probe.captured_at_ms,
            ir_probe.captured_at_ms,
            self.cross_check.max_time_skew_ms,
        ) {
            return self.cross_check_mismatch(username, auth_scope);
        }

        let mut rgb_frame = match frame_from_probe(rgb_probe) {
            Ok(frame) => frame,
            Err(err) => return self.cross_check_internal(username, auth_scope, err),
        };
        let mut ir_frame = match frame_from_probe(ir_probe) {
            Ok(frame) => frame,
            Err(err) => {
                rgb_frame.data.zeroize();
                return self.cross_check_internal(username, auth_scope, err);
            }
        };

        let mut rgb_probe = match self.run_inference_strict(&rgb_frame) {
            Ok(Some(probe)) => probe,
            Ok(None) => {
                rgb_frame.data.zeroize();
                ir_frame.data.zeroize();
                return self.cross_check_mismatch(username, auth_scope);
            }
            Err(err) => {
                rgb_frame.data.zeroize();
                ir_frame.data.zeroize();
                return self.cross_check_internal(username, auth_scope, err);
            }
        };
        let mut ir_probe = match self.run_inference_strict(&ir_frame) {
            Ok(Some(probe)) => probe,
            Ok(None) => {
                rgb_probe.embedding.zeroize();
                rgb_frame.data.zeroize();
                ir_frame.data.zeroize();
                return self.cross_check_mismatch(username, auth_scope);
            }
            Err(err) => {
                rgb_probe.embedding.zeroize();
                rgb_frame.data.zeroize();
                ir_frame.data.zeroize();
                return self.cross_check_internal(username, auth_scope, err);
            }
        };
        rgb_frame.data.zeroize();
        ir_frame.data.zeroize();

        let consistency = cross_stream_consistency(
            &rgb_probe,
            &ir_probe,
            &self.cross_check.homography,
            self.cross_check.max_position_offset_px,
            self.cross_check.min_identity_similarity,
        );
        if !consistency {
            rgb_probe.embedding.zeroize();
            ir_probe.embedding.zeroize();
            return self.cross_check_mismatch(username, auth_scope);
        }

        let response = self.compare_embedding(username, auth_scope, &rgb_probe.embedding);
        rgb_probe.embedding.zeroize();
        ir_probe.embedding.zeroize();
        response
    }

    fn cross_check_mismatch(&self, username: &str, auth_scope: AuthScope) -> ResponseEnvelope {
        self.record_match_failure(username);
        self.write_audit(
            username,
            auth_scope,
            AuditOutcome::Failure,
            AuditReason::Mismatch,
        );
        ResponseEnvelope::ok(Response::Match {
            result: MatchResult {
                matched: false,
                score: None,
                template_id: None,
            },
        })
    }

    fn cross_check_internal(
        &self,
        username: &str,
        auth_scope: AuthScope,
        err: ResponseEnvelope,
    ) -> ResponseEnvelope {
        self.record_match_failure(username);
        self.write_audit(
            username,
            auth_scope,
            AuditOutcome::Failure,
            AuditReason::Internal,
        );
        err
    }

    /// Lock the inference state, lazily loading SCRFD + ArcFace on first use,
    /// then detect+embed the largest face. Returns Ok(None) when no face is
    /// found in the frame.
    fn run_inference(
        &self,
        frame: &Frame,
    ) -> std::result::Result<Option<Vec<f32>>, ResponseEnvelope> {
        Ok(self
            .run_inference_details(frame, false)?
            .map(|probe| probe.embedding))
    }

    fn run_inference_strict(
        &self,
        frame: &Frame,
    ) -> std::result::Result<Option<InferenceProbe>, ResponseEnvelope> {
        self.run_inference_details(frame, true)
    }

    fn run_inference_details(
        &self,
        frame: &Frame,
        require_exactly_one_face: bool,
    ) -> std::result::Result<Option<InferenceProbe>, ResponseEnvelope> {
        let mut guard = self.inference.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(load_inference(&self.models).map_err(|e| {
                tracing::error!("cannot load inference models: {e}");
                ResponseEnvelope::error(ErrorCode::Internal, "broker inference unavailable")
            })?);
        }
        let state = guard.as_mut().expect("inference loaded above");

        let detections = state
            .detector
            .detect(frame, self.min_face_size)
            .map_err(|e| {
                tracing::warn!("detection error: {e}");
                ResponseEnvelope::error(ErrorCode::Internal, "face detection failed")
            })?;

        let detection = if require_exactly_one_face {
            if detections.len() != 1 {
                return Ok(None);
            }
            detections.into_iter().next().expect("len checked")
        } else if let Some(detection) = detections
            .into_iter()
            .max_by(|a, b| a.bbox.area().total_cmp(&b.bbox.area()))
        {
            detection
        } else {
            return Ok(None);
        };

        let embedding = state.embedder.extract(frame, &detection).map_err(|e| {
            tracing::warn!("embedding error: {e}");
            ResponseEnvelope::error(ErrorCode::Internal, "face embedding failed")
        })?;
        Ok(Some(InferenceProbe {
            detection,
            embedding,
        }))
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

fn load_inference(models: &ModelsConfig) -> Result<InferenceState> {
    tracing::debug!(path = %models.detector.display(), "loading detector");
    let detector = ScrfdDetector::load(&models.detector)
        .with_context(|| format!("cannot load detector at {}", models.detector.display()))?;
    tracing::debug!(path = %models.embedder.display(), "loading embedder");
    let embedder = ArcFaceEmbedder::load(&models.embedder)
        .with_context(|| format!("cannot load embedder at {}", models.embedder.display()))?;
    Ok(InferenceState { detector, embedder })
}

fn frame_from_probe(probe: &FrameProbe) -> std::result::Result<Frame, ResponseEnvelope> {
    if probe.width == 0 || probe.height == 0 {
        return Err(ResponseEnvelope::error(
            ErrorCode::BadRequest,
            "frame width and height must be non-zero",
        ));
    }
    let pixels = probe.width.saturating_mul(probe.height);
    if pixels > MAX_FRAME_PIXELS {
        return Err(ResponseEnvelope::error(
            ErrorCode::BadRequest,
            "frame geometry exceeds broker limit",
        ));
    }
    let pixels = pixels as usize;
    let expected = match probe.format {
        FrameFormat::Rgb8 | FrameFormat::Bgr8 => pixels.saturating_mul(3),
        FrameFormat::Gray8 => pixels,
    };
    if probe.bytes.len() != expected {
        return Err(ResponseEnvelope::error(
            ErrorCode::BadRequest,
            format!(
                "frame buffer size mismatch: got {}, expected {}",
                probe.bytes.len(),
                expected
            ),
        ));
    }

    let data = match probe.format {
        FrameFormat::Rgb8 => probe.bytes.clone(),
        FrameFormat::Bgr8 => {
            let mut out = Vec::with_capacity(probe.bytes.len());
            for chunk in probe.bytes.chunks_exact(3) {
                out.extend_from_slice(&[chunk[2], chunk[1], chunk[0]]);
            }
            out
        }
        FrameFormat::Gray8 => {
            let mut out = Vec::with_capacity(pixels * 3);
            for &y in &probe.bytes {
                out.extend_from_slice(&[y, y, y]);
            }
            out
        }
    };

    Ok(Frame {
        data,
        width: probe.width,
        height: probe.height,
    })
}

fn frames_are_synchronized(a_ms: u64, b_ms: u64, max_skew_ms: u64) -> bool {
    if a_ms == 0 || b_ms == 0 {
        return false;
    }
    a_ms.abs_diff(b_ms) <= max_skew_ms
}

fn cross_stream_consistency(
    rgb: &InferenceProbe,
    ir: &InferenceProbe,
    homography: &[f32; 9],
    max_position_offset_px: f32,
    min_identity_similarity: f32,
) -> bool {
    let rgb_centroid = landmark_centroid(rgb);
    let ir_centroid = landmark_centroid(ir);
    let Some(mapped_ir) = apply_homography(ir_centroid, homography) else {
        return false;
    };
    let dx = rgb_centroid.0 - mapped_ir.0;
    let dy = rgb_centroid.1 - mapped_ir.1;
    let position_offset = (dx * dx + dy * dy).sqrt();
    if !position_offset.is_finite() || position_offset > max_position_offset_px {
        return false;
    }

    let identity = cosine_similarity(&rgb.embedding, &ir.embedding);
    identity.is_finite() && identity >= min_identity_similarity
}

fn landmark_centroid(probe: &InferenceProbe) -> (f32, f32) {
    let mut x = 0.0;
    let mut y = 0.0;
    for (px, py) in probe.detection.landmarks.points {
        x += px;
        y += py;
    }
    (x / 5.0, y / 5.0)
}

fn apply_homography(point: (f32, f32), h: &[f32; 9]) -> Option<(f32, f32)> {
    let (x, y) = point;
    let denom = h[6] * x + h[7] * y + h[8];
    if !denom.is_finite() || denom.abs() < f32::EPSILON {
        return None;
    }
    let mapped_x = (h[0] * x + h[1] * y + h[2]) / denom;
    let mapped_y = (h[3] * x + h[4] * y + h[5]) / denom;
    if mapped_x.is_finite() && mapped_y.is_finite() {
        Some((mapped_x, mapped_y))
    } else {
        None
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
    use facegate_core::detection::{BoundingBox, Landmarks};
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
            min_face_size: 80,
            cooldown_after_failures: 10,
            cooldown_duration: Duration::from_secs(60),
            cross_check: CrossCheckPolicy {
                required: false,
                max_time_skew_ms: 50,
                max_position_offset_px: 40.0,
                min_identity_similarity: 0.55,
                homography: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            abuse: Arc::new(Mutex::new(AbuseState::default())),
            models: facegate_core::config::ModelsConfig {
                detector: std::path::PathBuf::new(),
                embedder: std::path::PathBuf::new(),
            },
            inference: Arc::new(Mutex::new(None)),
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
    fn match_rejects_non_root_callers() {
        // Match is admin-only since protocol v2 — non-root must go through
        // MatchFrame so the broker controls the embedding.
        let (_dir, state) = test_state();
        let response = state.dispatch(
            RequestEnvelope::new(Request::Match {
                username: "alice".to_owned(),
                auth_scope: IpcAuthScope::Session,
                probe_embedding: unit_vec(&[1.0, 0.0]),
            }),
            Some(PeerCredentials { uid: 1000 }),
        );
        let Response::Error(error) = response.response else {
            panic!("expected error for non-root Match");
        };
        assert_eq!(error.code, ErrorCode::Unauthorized);
    }

    #[test]
    fn match_frame_rejects_malformed_buffer() {
        let (_dir, state) = test_state();
        let response = state.dispatch(
            RequestEnvelope::new(Request::MatchFrame {
                username: "alice".to_owned(),
                auth_scope: IpcAuthScope::Session,
                frame: facegate_ipc::FrameProbe {
                    format: facegate_ipc::FrameFormat::Rgb8,
                    width: 4,
                    height: 4,
                    captured_at_ms: 1,
                    bytes: vec![0; 10], // expected 48 bytes
                },
            }),
            Some(PeerCredentials { uid: 0 }),
        );
        let Response::Error(error) = response.response else {
            panic!("expected error for malformed frame");
        };
        assert_eq!(error.code, ErrorCode::BadRequest);
    }

    #[test]
    fn match_frame_rejects_oversized_geometry() {
        let (_dir, state) = test_state();
        let response = state.dispatch(
            RequestEnvelope::new(Request::MatchFrame {
                username: "alice".to_owned(),
                auth_scope: IpcAuthScope::Session,
                frame: facegate_ipc::FrameProbe {
                    format: facegate_ipc::FrameFormat::Gray8,
                    width: 8192,
                    height: 8192,
                    captured_at_ms: 1,
                    bytes: vec![],
                },
            }),
            Some(PeerCredentials { uid: 0 }),
        );
        let Response::Error(error) = response.response else {
            panic!("expected error for oversized frame");
        };
        assert_eq!(error.code, ErrorCode::BadRequest);
    }

    #[test]
    fn frame_from_probe_bgr_swaps_channels() {
        let probe = facegate_ipc::FrameProbe {
            format: facegate_ipc::FrameFormat::Bgr8,
            width: 1,
            height: 1,
            captured_at_ms: 1,
            bytes: vec![10, 20, 30], // B=10, G=20, R=30
        };
        let frame = frame_from_probe(&probe).expect("decode");
        assert_eq!(frame.data, vec![30, 20, 10]); // RGB
    }

    #[test]
    fn frame_from_probe_gray_expands_to_rgb() {
        let probe = facegate_ipc::FrameProbe {
            format: facegate_ipc::FrameFormat::Gray8,
            width: 2,
            height: 1,
            captured_at_ms: 1,
            bytes: vec![42, 200],
        };
        let frame = frame_from_probe(&probe).expect("decode");
        assert_eq!(frame.data, vec![42, 42, 42, 200, 200, 200]);
    }

    #[test]
    fn cross_check_required_rejects_single_frame() {
        let (_dir, mut state) = test_state();
        state.cross_check.required = true;
        let response = state.dispatch(
            RequestEnvelope::new(Request::MatchFrame {
                username: "alice".to_owned(),
                auth_scope: IpcAuthScope::Session,
                frame: facegate_ipc::FrameProbe {
                    format: facegate_ipc::FrameFormat::Rgb8,
                    width: 1,
                    height: 1,
                    captured_at_ms: 1,
                    bytes: vec![0, 0, 0],
                },
            }),
            Some(PeerCredentials { uid: 0 }),
        );
        let Response::Match { result } = response.response else {
            panic!("expected match response");
        };
        assert!(!result.matched);
        assert_eq!(result.score, None);
    }

    #[test]
    fn cross_check_rejects_unsynchronized_frames() {
        let (_dir, mut state) = test_state();
        state.cross_check.required = true;
        let response = state.dispatch(
            RequestEnvelope::new(Request::MatchFramePair {
                username: "alice".to_owned(),
                auth_scope: IpcAuthScope::Session,
                rgb_frame: one_pixel_rgb(100),
                ir_frame: one_pixel_rgb(200),
            }),
            Some(PeerCredentials { uid: 0 }),
        );
        let Response::Match { result } = response.response else {
            panic!("expected match response");
        };
        assert!(!result.matched);
        assert_eq!(result.score, None);
    }

    #[test]
    fn cross_stream_consistency_rejects_position_mismatch() {
        let rgb = inference_probe((10.0, 10.0), unit_vec(&[1.0, 0.0]));
        let ir = inference_probe((100.0, 100.0), unit_vec(&[1.0, 0.0]));
        assert!(!cross_stream_consistency(
            &rgb,
            &ir,
            &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            40.0,
            0.55,
        ));
    }

    #[test]
    fn cross_stream_consistency_rejects_identity_mismatch() {
        let rgb = inference_probe((10.0, 10.0), unit_vec(&[1.0, 0.0]));
        let ir = inference_probe((12.0, 12.0), unit_vec(&[0.0, 1.0]));
        assert!(!cross_stream_consistency(
            &rgb,
            &ir,
            &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            40.0,
            0.55,
        ));
    }

    #[test]
    fn cross_stream_consistency_accepts_happy_path() {
        let rgb = inference_probe((40.0, 30.0), unit_vec(&[1.0, 0.0]));
        let ir = inference_probe((30.0, 25.0), unit_vec(&[0.98, 0.02]));
        let homography = [1.0, 0.0, 10.0, 0.0, 1.0, 5.0, 0.0, 0.0, 1.0];
        assert!(cross_stream_consistency(&rgb, &ir, &homography, 5.0, 0.55,));
    }

    fn one_pixel_rgb(captured_at_ms: u64) -> facegate_ipc::FrameProbe {
        facegate_ipc::FrameProbe {
            format: facegate_ipc::FrameFormat::Rgb8,
            width: 1,
            height: 1,
            captured_at_ms,
            bytes: vec![0, 0, 0],
        }
    }

    fn inference_probe(centroid: (f32, f32), embedding: Vec<f32>) -> InferenceProbe {
        InferenceProbe {
            detection: Detection {
                bbox: BoundingBox {
                    x1: centroid.0 - 5.0,
                    y1: centroid.1 - 5.0,
                    x2: centroid.0 + 5.0,
                    y2: centroid.1 + 5.0,
                    confidence: 1.0,
                },
                landmarks: Landmarks {
                    points: [centroid; 5],
                },
            },
            embedding,
        }
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
