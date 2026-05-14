# Facegate Threat Model

Facegate is a convenience authentication mechanism for local Linux machines. It
reduces friction for sudo, login, and screen unlock, but it does not replace a
strong password or hardware-backed authentication.

## Assets

- Enrolled biometric templates in `/var/lib/facegate/users`.
- Broker audit log in `/var/lib/facegate/audit.log`.
- Broker socket at `/run/facegate/broker.sock`.
- PAM trust decision made by `pam_facegate.so` via `facegate auth`.
- Camera frames and transient embeddings submitted for matching.

## Trust Boundary

Since v0.2.0 the broker is the biometric trust boundary. Clients capture frames
and submit them to `facegate-brokerd`; the broker runs detection, embedding,
matching, rate limiting, and audit logging.

Non-root clients do not read stored templates and do not decide match outcomes.
Root remains fully trusted.

## In-Scope Adversaries

Remote network:

- Assumption: no remote path exists.
- Mitigation: broker has no network listener; systemd sets `PrivateNetwork=yes`,
  `RestrictAddressFamilies=AF_UNIX`, and `IPAddressDeny=any`.

Same-host different UID:

- Goal: read templates, inject broker requests, or unlock another session.
- Mitigations: templates are owned by `facegate:facegate`; broker uses
  `SO_PEERCRED`; logind/polkit restrict session unlock operations.

Same UID:

- Goal: read the user's own templates or fabricate a match.
- Mitigations: templates are broker-owned, non-root `match` is forbidden, and
  non-root clients must submit `MatchFrame` or `MatchFramePair` frames. Broker
  rate limits and lockouts apply per UID and username.

Root:

- Root is out of scope. Root can edit PAM, read memory, replace binaries, and
  read or modify all files. Password authentication is the ultimate fallback.

## Current Non-Goals

- TPM-sealed templates at rest: tracked in
  `docs/security-issues/07-tpm-sealing.md` and issue #26.
- Full liveness/PAD model integration: tracked in
  `docs/security-issues/06-liveness-pad.md` and issue #25.
- Complete per-context policy engine beyond current scope-specific recognition
  defaults: tracked in `docs/security-issues/08-stricter-recognition-defaults.md`.

RGB+IR cross-check groundwork is implemented for v0.3.0, but it is not a full
PAD model and should be treated as defense-in-depth.

## Roadmap Mapping

- `00-biometric-template-leak-roadmap.md`: overall storage and broker hardening.
- `01-template-broker-service.md`: broker trust boundary.
- `02-template-storage-permissions-and-migration.md`: broker-owned template
  storage.
- `03-replace-direct-template-access-with-broker-ipc.md`: frame-based matching.
- `04-broker-runtime-hardening.md`: systemd sandboxing.
- `05-correct-security-docs-for-template-risk.md`: user-facing template risk.
- `06-liveness-pad.md`: PAD/liveness roadmap.
- `07-tpm-sealing.md`: template sealing roadmap.
- `08-stricter-recognition-defaults.md`: scope-specific matching policy.
- `09-dual-camera-cross-check.md`: RGB+IR presentation attack defense.

## Recovery Assumption

PAM changes can lock users out if tested carelessly. Operators should keep a
root shell open while changing PAM and use `facegate emergency-disable` or
`docs/recovery.md` if authentication breaks.
