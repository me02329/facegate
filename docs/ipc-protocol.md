# Facegate Broker IPC Protocol

Facegate clients talk to `facegate-brokerd` over a local Unix domain socket.
The protocol is JSON over newline-delimited frames and is defined in
`crates/facegate_ipc`.

## Transport

- Socket: `/run/facegate/broker.sock`
- Framing: one JSON request per line, one JSON response per line
- Protocol version: `5`
- Maximum request size: 24 MiB
- Maximum response size: 1 MiB
- Broker socket mode: `0666`
- Broker process: `facegate-brokerd.service`, running as `facegate:facegate`

The broad socket mode is intentional: authorization is done by the broker with
`SO_PEERCRED`, not by filesystem mode. The broker reads the peer UID for each
connection and applies per-request authorization rules.

## Envelope

Every request is wrapped:

```json
{"version":5,"request":{"type":"health"}}
```

Every response is wrapped:

```json
{"version":5,"response":{"type":"health","info":{"protocol_version":5,"broker_version":"0.3.0"}}}
```

If the version does not match, the broker returns `version_mismatch`.

## Authorization

- `uid=0` may call administrative and enrollment endpoints.
- Non-root users may call session-oriented match/list endpoints for their own
  username only.
- Non-root users cannot submit raw embeddings through `match`; they must submit
  live camera frames through `match_frame` or `match_frame_pair`.

The broker resolves user ownership with libc passwd lookups. Unauthorized
requests return `unauthorized` and never expose stored template vectors.

## Requests

- `health`: returns protocol and broker version.
- `users`: lists enrolled users and ownership/mode summaries. Root sees all
  users; non-root sees only their own account.
- `audit_recent`: returns recent audit events authorized for the caller.
- `match`: root-only embedding comparison endpoint used by admin calibration
  tooling.
- `match_frame`: submits one RGB frame for broker-side detection, embedding,
  and matching.
- `match_frame_pair`: submits synchronized RGB and IR frames for RGB+IR
  cross-check. **Only valid when the broker has
  `[camera.cross_check].enabled = true`.** A `match_frame_pair` sent to a
  broker without a cross-check policy is refused with `bad_request` — the
  broker does not silently fall back to single-frame matching, because that
  would hide a client/broker config mismatch. Clients that do not know
  whether the broker requires cross-check should send `match_frame`.
- `enroll`: root-only template enrollment.
- `list`: lists template metadata for one user.
- `remove`: root-only template deletion.

Stored embeddings are never returned by `list` or `users`.

## Frame Payloads

`FrameProbe` contains:

- `format`: `rgb8`, `bgr8`, or `gray8`
- `width`, `height`
- `captured_at_ms`: client capture timestamp in milliseconds since the
  UNIX epoch. **Zero is a reserved sentinel meaning "not provided"** —
  any `MatchFramePair` with `captured_at_ms == 0` on either side is
  rejected with `CrossCheckTimeSkew`, which lets legacy v2 clients fall
  back cleanly without bumping the protocol version. New clients MUST
  populate this from a real wall-clock source (any post-1970
  `SystemTime::now` in milliseconds is non-zero, so the sentinel never
  collides with real data).
- `bytes`: base64-encoded raw frame bytes

The broker rejects malformed geometry, oversized geometry above 4096 x 4096,
and byte buffers whose length does not match the declared format.

## Responses

- `health`
- `users`
- `audit`
- `match`
- `enrolled`
- `list`
- `removed`
- `error`

`match` returns only a decision, optional score, optional matched template ID,
and coarse reason. It does not return embeddings.

## Errors

- `bad_request`: malformed JSON, invalid frame envelope, or unsupported payload
- `version_mismatch`: client and broker protocol versions differ
- `unauthorized`: peer UID is not authorized for the request
- `not_enrolled`: no template exists for the requested user/scope
- `rate_limited`: peer exceeded the broker request rate
- `locked_out`: repeated failed matches triggered temporary lockout
- `unsupported`: reserved for unsupported protocol features
- `internal`: storage, inference, or audit failures

## Zeroization And Audit

The broker zeroizes:

- submitted probe embeddings after `match`;
- decoded frame bytes after `match_frame` / `match_frame_pair`;
- loaded template embeddings after comparison/list processing.

The audit log records timestamp, username, auth scope, coarse outcome, and
coarse reason. It does not log frames, embeddings, or similarity scores.

## Worked Example

Health request:

```json
{"version":5,"request":{"type":"health"}}
```

Frame match request shape:

```json
{
  "version": 5,
  "request": {
    "type": "match_frame",
    "username": "alice",
    "auth_scope": "session",
    "frame": {
      "format": "rgb8",
      "width": 640,
      "height": 360,
      "captured_at_ms": 1778750000000,
      "bytes": "..."
    }
  }
}
```

Match response shape:

```json
{
  "version": 5,
  "response": {
    "type": "match",
    "result": {
      "matched": true,
      "score": 0.71,
      "template_id": 2,
      "reason": "matched"
    }
  }
}
```
