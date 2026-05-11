# [Security] Add `facegate-brokerd` to own and match biometric templates

## Priority

Critical / P0.

## Problem

`facegate auth` and `facegate-watch` currently call `TemplateStore` directly. For session unlock, the template file is deliberately owned by the enrolled user so the user service can read it. This makes template exfiltration trivial for any same-UID process.

## Proposed design

Introduce a new broker:

```text
facegate-brokerd
  UID/GID: facegate:facegate
  socket: /run/facegate/broker.sock
  owns: /var/lib/facegate/users
```

Clients:

```text
pam_facegate.so -> facegate auth -> broker
facegate-watch -> broker
sudo facegate add/list/remove/test -> broker
```

The broker is the only component allowed to read or write raw enrolled embeddings.

## IPC API

Create a small `facegate_ipc` crate with versioned request/response types.

Minimum MVP operations:

- `Match { username, auth_scope, probe_embedding } -> MatchResult`
- future-compatible: `MatchFrame { username, auth_scope, frame } -> MatchResult`
- `Enroll { username, label, scope, embedding } -> EnrolledTemplateSummary`
- `List { username } -> Vec<EnrolledTemplateSummary>`
- `Remove { username, template_id } -> ()`
- `Health -> BrokerInfo`

Important: do not return raw enrolled embeddings from any broker API.

## Authorization rules

- `Match` for session scope may be called by the same UID as `username`.
- `Match` for sudo scope may be called by root-owned PAM/helper context, or by a tightly constrained helper identity if introduced.
- `Enroll`, `Remove`, and cross-user `List` require root.
- Same-user `List` may return metadata only: id, label, scope, created_at. Never return vectors.
- The broker must verify peer credentials on the Unix socket with kernel credentials (`SO_PEERCRED` or the Rust equivalent).
- Usernames must continue using the existing strict username validation.

## Anti-abuse

These controls do not mitigate the crafted-embedding attack described below. They are hygiene against brute-force, spray, accidental loops, and local DoS.

- Add per-peer and per-username rate limiting for `Match`.
- Add a sliding-window failure counter per username.
- Add a temporary lockout after repeated failed matches for the same username.
- Log rate-limit and lockout events without logging embeddings or full request payloads.
- Make lockout behavior configurable enough to avoid permanently locking out legitimate users.

## Camera model

Keep camera capture in the existing clients for the MVP.

Reason: Linux logind grants `/dev/video*` access to the active user session, not to an arbitrary system user. If the broker tried to capture directly, it would either need broad camera permissions or custom udev rules, both of which are worse operationally.

MVP flow:

```text
client opens camera -> client extracts probe embedding -> broker compares probe against stored templates -> broker returns yes/no
```

This protects stored templates from passive exfiltration. Liveness and synthetic-input resistance are separate work.

## Security trade-off: probe embedding vs raw frame

The MVP `Match` API trusts the client-provided probe embedding. This is enough to stop passive template exfiltration because the broker never exposes enrolled embeddings, but it is not equivalent to Windows Hello's end-to-end biometric pipeline.

A same-UID attacker who can connect to the broker and can compute an ArcFace-compatible embedding from a public photo may submit a crafted `probe_embedding` without using the camera. That attacker still cannot read stored templates, but the broker is not proving that the probe came from a live camera capture.

Rate limiting and lockout do not meaningfully slow this attack if the attacker can produce a single high-quality matching embedding. They only protect against random probing, spray, and denial-of-service patterns.

Longer-term, keep the IPC protocol compatible with a frame-based path:

```text
client captures frame -> broker runs SCRFD + ArcFace + liveness -> broker compares -> broker returns yes/no
```

That upgrade is the real mitigation for crafted client embeddings and is required for broker-side liveness and stronger Windows Hello-style semantics. The MVP should document this limitation clearly and avoid claiming full Windows Hello equivalence.

## Implementation notes

- Add workspace crate `crates/facegate_ipc`.
- Add workspace crate or binary for `facegate-brokerd`.
- Move matching decision logic into broker-owned code.
- Keep `facegate_core::storage::TemplateStore`, but make production callers access it only from the broker.
- Add integration tests for socket auth and metadata-only list responses.

## Acceptance criteria

- No non-broker production command reads `embeddings.json` directly.
- `facegate auth` and `facegate-watch` only receive match decisions, not enrolled vectors.
- The broker rejects unauthorized peer UIDs.
- The broker rate-limits abusive match attempts and temporarily locks out repeated failures per username.
- The broker never exposes enrolled embeddings over IPC.
- The socket protocol is versioned so future changes fail closed.
- The API leaves a clear upgrade path for frame-based broker matching.
