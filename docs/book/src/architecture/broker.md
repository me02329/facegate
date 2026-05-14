# The broker daemon

`facegate-brokerd` is the trust boundary. Everything below the broker
runs unprivileged; everything that touches templates or runs ML
inference runs *inside* the broker.

## Identity and storage

- System user / group: `facegate:facegate` (created at install time,
  `useradd --system --no-create-home --home-dir /var/lib/facegate`).
- Templates: `/var/lib/facegate/users/<username>/embeddings.json`,
  mode `0600`, owner `facegate:facegate`.
- Audit log: `/var/lib/facegate/audit.log`, mode `0600`, owner
  `facegate:facegate`. Contains coarse outcome + reason, no
  embeddings, no scores, no frames.
- Socket: `/run/facegate/broker.sock`, mode `0666` (peer credentials
  enforced server-side, not via filesystem ACL).

## Systemd hardening

The unit file shipped at
`/usr/lib/systemd/system/facegate-brokerd.service` applies:

```ini
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=no            # needs V4L2 access at runtime when matching
PrivateNetwork=yes
ProtectKernelModules=yes
ProtectKernelTunables=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectProc=invisible
RestrictAddressFamilies=AF_UNIX
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallFilter=~@privileged @resources @mount @cpu-emulation @debug @raw-io @reboot @swap @obsolete
CapabilityBoundingSet=
AmbientCapabilities=
IPAddressDeny=any
ReadWritePaths=/var/lib/facegate /run/facegate
```

If you change these defaults, run `systemd-analyze security
facegate-brokerd.service` to spot regressions.

## Request handling

For every request:

1. Read the JSON envelope from the Unix socket.
2. Check `version` matches `PROTOCOL_VERSION`; reject `VersionMismatch`
   if not.
3. Resolve `SO_PEERCRED` to a UID. Refuse if missing.
4. Authorise based on the request variant and peer UID (see
   [Architecture overview](./overview.md#surfaces-by-user)).
5. Dispatch to the handler. For `MatchFrame` / `MatchFramePair`:
   - Validate declared geometry against bounds (max 4096², buffer
     length must equal `width × height × bpp`).
   - Run SCRFD on the frame.
   - Reject if zero or multiple faces are detected.
   - Run ArcFace on the canonical crop.
   - Compare against every template for `(username, auth_scope)`.
   - Apply rate limiting, lockout, cooldown.
   - Zeroise the probe embedding and any loaded templates.
   - Append a coarse audit entry (success/failure + reason).
6. Send the response envelope.

## What the broker does **not** do

- It does not accept a precomputed embedding from a non-root caller.
  The legacy `Match` endpoint (v1 protocol) is `uid=0`-only since
  v0.2.0, blocking the synthetic-embedding bypass available to any
  same-UID process under v1.
- It does not log frames, scores, or embeddings — the audit log only
  carries (timestamp, username, scope, outcome, reason).
- It does not open the network. `PrivateNetwork=yes` +
  `RestrictAddressFamilies=AF_UNIX` + `IPAddressDeny=any` make this
  belt-and-braces.

## Operator surfaces

```sh
facegate broker status              # service + socket + audit + storage
facegate broker health              # IPC ping → protocol/version
facegate broker logs --lines 200    # journal lines for the unit
sudo facegate broker restart        # restart the service
sudo facegate broker repair-permissions  # re-apply ownership / modes
```

The first two are unprivileged; the latter three require root.
