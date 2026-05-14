# Security Policy

Facegate is a Linux PAM and biometric authentication tool. Bugs in this
project can lock users out of their machine, weaken `sudo`, or expose
biometric data, so we take security reports seriously.

## Supported versions

Only the latest published minor release receives security fixes. While
the IPC protocol stabilises (pre-1.0), we may also backport critical
fixes to the previous minor on a best-effort basis.

| Version | Status                                                  |
| ------- | ------------------------------------------------------- |
| 0.2.x   | Supported (active development, security fixes).         |
| 0.1.x   | Best-effort backports of critical fixes only.           |
| < 0.1   | Unsupported. Upgrade to 0.2.x.                          |

Upgrades between minors may require running the package postinstall to
migrate template ownership (`facegate:facegate`) and to refresh
`/var/lib/facegate/audit.log`. The CLI and the broker must be upgraded
together — the IPC protocol is versioned and a client built against an
older protocol is rejected with `VersionMismatch`.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security reports.

Preferred channel: GitHub private vulnerability reporting on this repo —

  https://github.com/me02329/facegate/security/advisories/new

Include, where possible:

- a description of the issue and its impact (what does an attacker gain?);
- the affected Facegate version (`facegate --version`) and Linux distro;
- a minimal reproducer or proof-of-concept;
- whether the report involves authenticated, same-UID, or remote access.

If GitHub private reporting is unavailable to you, contact the
maintainer at `github@martial.aleeas.com` with subject prefix
`[facegate-security]` and we will move the discussion to a private
channel.

### What to expect

- **Acknowledgement**: within 7 days.
- **Triage and severity assessment**: within 14 days.
- **Coordinated disclosure window**: 90 days from acknowledgement
  before public disclosure. We will negotiate an extension if a fix is
  in progress and the issue is non-trivial to patch.
- **Credit**: reporters are credited in the release notes and the
  relevant `docs/security-issues/*.md` unless they request anonymity.

## What is in scope

We consider the following classes of issues in scope:

- PAM module behaviour that allows authentication without a valid face
  match or that crashes / hangs PAM.
- Same-UID exfiltration of enrolled biometric templates or matching
  state held by `facegate-brokerd`.
- Bypass of the broker's match logic: synthetic embeddings, replay,
  protocol injection, etc.
- Watch-daemon misbehaviour that allows another local user to trigger
  or hijack an unlock.
- Misuse of polkit / D-Bus / logind interactions in either daemon.
- Packaging issues that produce insecure file permissions, leave the
  broker disabled, or skip checksum verification on downloaded models
  (see also [issue #13][issue-13]).
- Recovery and rollback failures: a broken PAM edit that cannot be
  reverted with `session-auth --off` or `facegate emergency-disable`
  (planned, [#34][issue-34]).

## What is out of scope (for now)

These limitations are publicly documented and tracked in
[`docs/security-issues/`](docs/security-issues):

- **Liveness / presentation attack detection.** A high-quality photo or
  replay can still match a single-camera capture (`06-liveness-pad.md`,
  [#25][issue-25]).
- **Dual-stream RGB+IR cross-check.** Not yet implemented on laptops
  with both sensors (`09-dual-camera-cross-check.md`, [#28][issue-28],
  planned for v0.3.0).
- **TPM sealing of templates at rest.** Templates are protected by
  Unix file permissions but not yet sealed to platform state
  (`07-tpm-sealing.md`, [#26][issue-26]).
- **Threshold tuning** is the operator's responsibility; `facegate
  calibrate` helps but does not enforce a minimum. False-accept rates
  with overly permissive thresholds are not a vulnerability per se.

Reports about these limitations are welcome as feature requests, but
they do not qualify for coordinated disclosure.

## If you suspect a live compromise

If you believe a Facegate install on your own machine has been
exploited:

1. **Keep a root shell open** before doing anything else. A wrong move
   in `/etc/pam.d/` can lock you out.
2. Disable face auth quickly:

   ```bash
   sudo facegate emergency-disable --dry-run
   sudo facegate emergency-disable
   sudo facegate session-auth     # fallback toggle if needed
   sudo systemctl disable --now facegate-brokerd.service
   systemctl --user disable --now facegate-watch.service
   ```

3. If the emergency command cannot run, restore
   `/etc/pam.d/*.facegate.*.bak` files manually.
4. Capture evidence before reinstalling:
   - `journalctl -u facegate-brokerd.service`;
   - `/var/lib/facegate/audit.log`;
   - `stat /var/lib/facegate/users/<your-user>/embeddings.json`;
   - the output of `sudo facegate status` and `sudo facegate doctor`.
5. Report the incident using the channel above.

A full PAM recovery guide (including chroot / live-USB scenarios) lives
at [`docs/recovery.md`](docs/recovery.md).

## Threat model

A short overview of the threat model lives in this document; the full
version is tracked as [#38][issue-38] and will move to
`docs/threat-model.md` when written.

**Assets**: enrolled biometric templates, the audit log, the broker
socket, the PAM trust decision.

**In-scope adversaries**:

- *Remote network*: assumed to have no path to Facegate (no listening
  socket, no network exposure on `facegate-brokerd`).
- *Same-host different-UID*: cannot read templates (broker-owned),
  cannot inject D-Bus `Lock` signals (logind-only sender), cannot
  call `Unlock()` on someone else's session (polkit-enforced).
- *Same-UID*: cannot read templates, cannot fabricate a match via a
  precomputed embedding (`Match` restricted to uid=0; non-root must
  use `MatchFrame` with a real camera frame), is rate-limited by the
  broker per UID and per username.
- *Root*: out of scope. Root can do anything; password auth is the
  ultimate fallback.

**Explicit non-goals for the current release**: liveness / PAD,
dual-camera cross-check, TPM-sealed templates at rest. See the issues
linked above.

## See also

- [`README.md`](README.md) — Security section.
- [`CHANGELOG.md`](CHANGELOG.md) — release notes including the v0.2.0
  trust-boundary move.
- [`docs/security-issues/`](docs/security-issues) — public hardening
  roadmap.

[issue-13]: https://github.com/me02329/facegate/issues/13
[issue-25]: https://github.com/me02329/facegate/issues/25
[issue-26]: https://github.com/me02329/facegate/issues/26
[issue-28]: https://github.com/me02329/facegate/issues/28
[issue-34]: https://github.com/me02329/facegate/issues/34
[issue-37]: https://github.com/me02329/facegate/issues/37
[issue-38]: https://github.com/me02329/facegate/issues/38
