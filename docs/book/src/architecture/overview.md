# Architecture overview

Facegate is a workspace of five Rust crates plus a PAM module:

| Crate | Role |
|---|---|
| `facegate_brokerd` | Privileged system daemon. Owns templates, runs SCRFD + ArcFace, validates IPC peers via `SO_PEERCRED`, writes the audit log. |
| `facegate_cli` | The `facegate` binary — CLI subcommands and the Ratatui TUI. Also implements the `watch` and `auth` helper subcommands. |
| `facegate_core` | V4L2 capture, ONNX Runtime loading helpers, config schema, shared types. Linked into the broker for full inference; linked into the CLI only for capture. |
| `facegate_ipc` | Versioned JSON-over-Unix-socket protocol (currently v5) shared between clients and the broker. Defines `Request`, `Response`, audit events, `MatchFrame`, `MatchFramePair`. |
| `pam_facegate` | The PAM module loaded by `pam_unix.so` peers. Spawns `facegate auth` and reads its exit code. |

## Process model

```text
┌───────────────────────────┐         ┌─────────────────────────────┐
│   sudo / login / lock     │         │   facegate-brokerd.service  │
│  ┌─────────────────────┐  │         │  ┌───────────────────────┐  │
│  │  pam_facegate.so    │  │         │  │  SCRFD + ArcFace      │  │
│  │  (small, no ML)     │  │         │  │  (ONNX Runtime)       │  │
│  └─────────┬───────────┘  │         │  └───────────┬───────────┘  │
│            │ spawns       │  IPC    │              │              │
│  ┌─────────▼───────────┐  │  JSON   │  ┌───────────▼───────────┐  │
│  │  facegate auth      │◄─┼─────────┼─►│  Request handler      │  │
│  │  (capture + send)   │  │  AF_UNIX│  │  (peer-uid checks)    │  │
│  └─────────────────────┘  │         │  └───────────────────────┘  │
└───────────────────────────┘         └─────────────────────────────┘

           Same user UID                    facegate:facegate
           V4L2 frame only                  Templates + audit log
                                            /run/facegate/broker.sock
```

The headline property is that **no user-mode process outside the
broker ever sees a stored template or a precomputed embedding**. The
PAM helper and the watch daemon both capture a raw frame, base64-
encode it into a `MatchFrame` envelope, and let the broker do its own
detection + embedding + match.

## Surfaces by user

| Caller | What it can do over IPC |
|---|---|
| `uid != 0`, same user | `Health`, `MatchFrame` for their own user, `AuditRecent` for their own user, `ListTemplates` for their own user |
| `uid != facegate's own user` | Cannot read template files directly (mode `0600`, gid `facegate`) |
| `uid = 0` | All of the above, plus enrol/remove templates, list every user, repair permissions |

Rate limiting and lockout live on the broker side per peer UID and per
target username; a same-UID attacker spamming `MatchFrame` cannot
exhaust attempts for a *different* user.

## Next steps

- [The broker daemon](./broker.md) — systemd hardening profile,
  request handling, audit log format.
- [IPC protocol](./ipc.md) — message shapes, versioning, base64
  envelope for frames.
- [Screen unlock flow](./screen-unlock.md) — how the watch daemon
  reacts to D-Bus Lock signals.
