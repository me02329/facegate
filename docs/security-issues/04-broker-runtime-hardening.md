# [Security] Harden `facegate-brokerd` systemd service and IPC surface

## Priority

High, after the broker MVP works.

## Problem

Moving templates into a broker creates the right trust boundary, but the broker becomes a high-value local service. It should run with the smallest practical filesystem, syscall, and privilege surface.

## systemd service requirements

Add:

```ini
[Service]
User=facegate
Group=facegate
RuntimeDirectory=facegate
RuntimeDirectoryMode=0750
StateDirectory=facegate
UMask=0077
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/facegate
RestrictAddressFamilies=AF_UNIX
LockPersonality=yes
MemoryDenyWriteExecute=yes
RestrictRealtime=yes
SystemCallArchitectures=native
CapabilityBoundingSet=
AmbientCapabilities=
SystemCallFilter=@system-service
SystemCallFilter=~@mount @debug @cpu-emulation @obsolete @raw-io @reboot @swap @privileged
SystemCallErrorNumber=EPERM
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
ProtectProc=invisible
ProcSubset=pid
RestrictNamespaces=yes
RestrictSUIDSGID=yes
PrivateNetwork=yes
IPAddressDeny=any
LimitCORE=0
```

Tune these as needed for ONNX Runtime if the broker loads models in the future. In the MVP, the broker should not need camera devices or model files if clients send probe embeddings.

## Socket requirements

- Socket path: `/run/facegate/broker.sock`
- Socket owner: `facegate:facegate`
- Socket mode: `0660` or stricter, with authorization enforced by peer credentials.
- No TCP listener.
- No abstract namespace socket.
- Message size limits to avoid memory DoS.
- Request timeout limits.
- Structured errors that do not leak stored template values.

## Process hardening

- Disable core dumps for the broker with `LimitCORE=0`.
- Make embedding logging impossible by construction, not merely discouraged. Wrap embedding values in a redacting type such as `secrecy::Secret<T>` or an equivalent local type whose `Debug` output is `[REDACTED]`.
- Add tests or compile-time checks that prevent raw embedding containers from implementing useful `Debug` output.
- Zeroize embedding buffers systematically after use, for example with `zeroize::Zeroizing<Vec<f32>>` or an equivalent wrapper. Avoid "where practical" exceptions for raw template and probe buffers.
- Keep broker APIs metadata-only unless a raw vector is strictly required internally.

## Acceptance criteria

- `systemd-analyze security facegate-brokerd.service` output is documented in the PR.
- Broker has no camera device access in the MVP.
- Broker has no network address family access.
- Crashes do not produce core dumps containing embeddings.
- Logs never contain raw embedding vectors.
- Raw embedding buffers are redacted in debug output and zeroized after use.
