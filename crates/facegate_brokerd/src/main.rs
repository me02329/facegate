use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use facegate_ipc::{
    encode_response, BrokerInfo, ErrorCode, Request, RequestEnvelope, Response, ResponseEnvelope,
    PROTOCOL_VERSION,
};
use std::os::unix::fs::FileTypeExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

const DEFAULT_SOCKET_PATH: &str = "/run/facegate/broker.sock";
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let socket_path = socket_path();
    run(socket_path).await
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

async fn run(socket_path: PathBuf) -> Result<()> {
    prepare_socket_path(&socket_path)?;
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("cannot bind {}", socket_path.display()))?;

    tracing::info!(socket = %socket_path.display(), "facegate broker listening");

    loop {
        let (stream, _addr) = listener.accept().await.context("accept failed")?;
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream).await {
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

async fn handle_client(stream: UnixStream) -> Result<()> {
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
        Ok(envelope) => dispatch(envelope),
        Err(_) => ResponseEnvelope::error(ErrorCode::BadRequest, "invalid request JSON"),
    };
    write_response(reader.get_mut(), response).await
}

fn dispatch(envelope: RequestEnvelope) -> ResponseEnvelope {
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
        Request::Match { .. }
        | Request::MatchFrame { .. }
        | Request::Enroll { .. }
        | Request::List { .. }
        | Request::Remove { .. } => ResponseEnvelope::error(
            ErrorCode::Unsupported,
            "broker operation is not implemented yet",
        ),
    }
}

async fn write_response(stream: &mut UnixStream, response: ResponseEnvelope) -> Result<()> {
    let encoded = encode_response(&response).context("cannot encode response")?;
    stream
        .write_all(&encoded)
        .await
        .context("cannot write response")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_response_uses_current_protocol() {
        let response = dispatch(RequestEnvelope::new(Request::Health));
        let Response::Health { info } = response.response else {
            panic!("expected health response");
        };
        assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn rejects_protocol_mismatch() {
        let response = dispatch(RequestEnvelope {
            version: PROTOCOL_VERSION + 1,
            request: Request::Health,
        });
        let Response::Error(error) = response.response else {
            panic!("expected error response");
        };
        assert_eq!(error.code, ErrorCode::VersionMismatch);
    }
}
