//! Versioned IPC types shared by Facegate clients and the local broker.
//!
//! The broker API must never return enrolled embedding vectors. Clients may
//! send probe/enrollment embeddings in the MVP, but stored templates stay
//! broker-owned and are represented outside the broker as metadata only.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;

pub type Embedding = Vec<f32>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthScope {
    Sudo,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateScope {
    Sudo,
    Session,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrolledTemplateSummary {
    pub id: u32,
    pub label: String,
    pub created_at: String,
    pub scope: TemplateScope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchResult {
    pub matched: bool,
    pub score: Option<f32>,
    pub template_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerInfo {
    pub protocol_version: u16,
    pub broker_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub version: u16,
    pub request: Request,
}

impl RequestEnvelope {
    pub fn new(request: Request) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Health,
    Match {
        username: String,
        auth_scope: AuthScope,
        probe_embedding: Embedding,
    },
    MatchFrame {
        username: String,
        auth_scope: AuthScope,
        frame: FrameProbe,
    },
    Enroll {
        username: String,
        label: String,
        scope: TemplateScope,
        embedding: Embedding,
    },
    List {
        username: String,
    },
    Remove {
        username: String,
        template_id: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameProbe {
    pub format: FrameFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameFormat {
    Rgb8,
    Bgr8,
    Gray8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub version: u16,
    pub response: Response,
}

impl ResponseEnvelope {
    pub fn ok(response: Response) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            response,
        }
    }

    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::ok(Response::Error(BrokerError {
            code,
            message: message.into(),
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Health {
        info: BrokerInfo,
    },
    Match {
        result: MatchResult,
    },
    Enrolled {
        template: EnrolledTemplateSummary,
    },
    List {
        templates: Vec<EnrolledTemplateSummary>,
    },
    Removed,
    Error(BrokerError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    VersionMismatch,
    Unauthorized,
    NotEnrolled,
    RateLimited,
    LockedOut,
    Unsupported,
    Internal,
}

pub fn encode_response(response: &ResponseEnvelope) -> serde_json::Result<Vec<u8>> {
    let mut out = serde_json::to_vec(response)?;
    out.push(b'\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips() {
        let req = RequestEnvelope::new(Request::Match {
            username: "alice".to_owned(),
            auth_scope: AuthScope::Session,
            probe_embedding: vec![0.1, 0.2],
        });

        let json = serde_json::to_string(&req).expect("serialize");
        let decoded: RequestEnvelope = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(decoded, req);
    }

    #[test]
    fn list_response_exposes_metadata_only() {
        let response = ResponseEnvelope::ok(Response::List {
            templates: vec![EnrolledTemplateSummary {
                id: 7,
                label: "front".to_owned(),
                created_at: "2026-05-11T00:00:00Z".to_owned(),
                scope: TemplateScope::Both,
            }],
        });

        let json = serde_json::to_string(&response).expect("serialize");
        assert!(json.contains("front"));
        assert!(!json.contains("embedding"));
    }
}
