# Facegate

> Local, broker-isolated face authentication for Linux. PAM-integrated.
> Screen-unlock daemon. RGB+IR liveness signal. No cloud, no telemetry.

Facegate lets you authenticate with your face for `sudo`, login sessions,
and screen lock on Linux. It runs entirely on-device — the ML pipeline
(YuNet face detection + AuraFace embeddings via ONNX Runtime, both under
permissive licences since v0.4.0) lives inside a dedicated, sandboxed
system daemon (`facegate-brokerd`) that owns all biometric templates.

## What this site covers

- **[Getting started](./getting-started/installation.md)** — install
  Facegate from a package, set up your first user, wire it into PAM.
- **[Architecture](./architecture/overview.md)** — how the broker, the
  PAM helper, and the screen-unlock daemon talk to each other.
- **[Security](./security/threat-model.md)** — threat model, recovery
  procedures, and roadmap items that are not yet shipped.
- **[Reference](./reference/configuration.md)** — every config key,
  every CLI subcommand, every supported camera format.
- **[FAQ](./faq.md)** — the questions users actually ask, including
  *"why doesn't it work in the dark?"* and *"is this Windows Hello?"*

## Status

Facegate is **alpha** software. The IPC protocol (currently v5) is still
evolving; pin both the broker and the CLI together when you upgrade. The
trust boundary in `facegate-brokerd` is the headline v0.2.0 achievement;
the v0.3.0 release added operator tooling (broker subcommands, emergency
disable, admin user list), scope-specific recognition policy, and an
optional RGB+IR cross-check used as a liveness *signal* (not a full
presentation attack detection model — that work continues in
[#25][issue-25]).

## Source and tracking

- Repository: <https://github.com/me02329/facegate>
- Issue tracker: <https://github.com/me02329/facegate/issues>
- Security disclosure: see [`SECURITY.md`][security-md] in the repo —
  GitHub private vulnerability reporting + email fallback.

[issue-25]: https://github.com/me02329/facegate/issues/25
[security-md]: https://github.com/me02329/facegate/blob/master/SECURITY.md
