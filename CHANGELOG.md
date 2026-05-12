# Changelog

All notable changes to Facegate are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
on a best-effort basis while the IPC protocol stabilises.

## [Unreleased]

In-progress work targeting **v0.3.0**. Focus so far: packaging
reliability, install-time correctness, contributor ergonomics, and a
documented security disclosure channel. The bigger features tracked
for this release (dual-camera RGB+IR cross-check, broker subcommands,
emergency PAM rollback, liveness PAD groundwork) are still open.

### Added

- `SECURITY.md` with a supported-versions table, a private disclosure
  channel (GitHub private vulnerability reporting + email fallback),
  acknowledgement / triage / disclosure windows (7 / 14 / 90 days),
  an in-scope vs out-of-scope list, a live-compromise runbook, and a
  threat-model summary cross-referencing the roadmap (#32).
- `rust-toolchain.toml` pinning the workspace at `1.95.0` with
  `rustfmt` and `clippy`; the CI workflow installs the same version
  explicitly so contributors and CI no longer drift (#43).
- `.editorconfig` covering Rust, TOML/YAML/JSON, Markdown, shell, and
  Makefiles — Markdown keeps its trailing whitespace to preserve
  two-space line breaks (#44).

### Changed

- **Package postinstall hardened** (`packaging/nfpm/scripts/postinstall.sh`):
  `set -euo pipefail` + `umask 077` at the top; `/var/lib/facegate/audit.log`
  is created atomically via `install(1)` so there is no
  `root:root 0644` window; `curl` runs with `--fail` so HTTP errors
  aren't saved as fake archives; `sha256sum` is now mandatory and a
  missing / mismatching checksum aborts the install; `unzip`
  availability is checked before model extraction; `systemctl` errors
  are surfaced (only suppressed when systemd is genuinely absent);
  upgrades `try-restart` `facegate-brokerd.service`; the interactive
  default for the ONNX Runtime / model downloads flipped from "yes"
  to "no" (Ctrl-D no longer triggers a 400 MB pull); the template
  migration takes exclusive control of `/var/lib/facegate/users`
  before traversal and refuses to touch trees containing symlinks /
  sockets / FIFOs / device nodes; `useradd` records
  `--home-dir /var/lib/facegate` for clean auditing (refs #13).
- **`install-dev.sh` brought to parity with the broker architecture**:
  installs `facegate-brokerd` and its systemd unit, creates the
  `facegate:facegate` system user/group, migrates template storage
  ownership to the broker, creates the audit log atomically, and
  enables / `try-restart`s `facegate-brokerd.service`. The old
  `chown -R root:root /var/lib/facegate` that fought the v0.2.0
  layout is gone (#29).
- **`facegate(1)` man page refreshed for v0.2.0**: title bumped to
  0.2.0, the broker becomes the trust boundary in DESCRIPTION, the
  watch-daemon "How it works" section reflects `MatchFrame` (no more
  in-daemon SCRFD/ArcFace), the manual PAM setup example uses the
  absolute path `/usr/lib/security/pam_facegate.so`, FILES gains
  `facegate-brokerd`, `facegate-brokerd.service`,
  `/run/facegate/broker.sock`, the new `facegate:facegate` ownership
  on `embeddings.json` and `audit.log`, SECURITY NOTES gains a "Trust
  boundary: the broker" sub-section, the PAM helper timeout is
  documented as 25 s (was 45 s), the same-UID attacker paragraph and
  the Windows-Hello comparison are rewritten to reflect what v0.2.0
  actually achieved vs what is still tracked (liveness PAD,
  dual-camera, TPM sealing) (#31).
- **`.gitignore` expanded** to cover the `dist/` output of
  `scripts/package-nfpm.sh`, `*.deb` / `*.rpm` / `*.pkg.tar.zst` at
  the repo root, backup files left by `session-auth` (`*.bak`,
  `*.orig`, `*~`), local logs and `/tmp/` scratch, and common
  OS / editor noise (#45).

### Fixed

- nFPM package manifest declared `license: MIT` while the repo and
  every crate are GPL-3.0-or-later. The produced `.deb` / `.rpm` /
  `.pkg.tar.zst` now advertise the license they actually ship under
  (#30).

## [0.2.0] — 2026-05-11

This release moves the biometric trust boundary into a dedicated broker
daemon. Stored templates leave the enrolled user's filesystem ownership, the
match decision moves out of client processes, and a same-UID attacker can no
longer bypass live capture by submitting a precomputed embedding.

### Added

- **`facegate-brokerd`**, a system daemon owned by a dedicated `facegate`
  user, with systemd hardening (`NoNewPrivileges`, `MemoryDenyWriteExecute`,
  `ProtectProc=invisible`, seccomp filter, no caps, no network, AF_UNIX only).
- **`facegate_ipc`** crate defining the versioned JSON-over-Unix-socket
  protocol between clients and the broker. Peer credentials enforced via
  `SO_PEERCRED`.
- **Broker-side `MatchFrame`**: the client sends a raw camera frame and the
  broker runs SCRFD + ArcFace + match itself. Frame bytes and derived
  embeddings are zeroised after use; geometry and buffer-size bounds are
  validated before allocation.
- **Privacy-preserving audit log** at `/var/lib/facegate/audit.log` (coarse
  outcome and reason, no embeddings, no scores, no frames). Surfaced via
  `facegate status`.
- **`facegate status`** command summarising config, broker reachability,
  recent audit events, model and template presence.
- **`facegate setup`** guided enrolment + PAM wiring flow.
- **`facegate calibrate`** command for tuning the recognition threshold from
  observed match scores.
- **Per-template scopes** (`sudo`, `session`, `both`). Enrolment allows the
  operator to choose which auth flows a template applies to.
- **Configurable cooldown** after repeated failed matches
  (`[security].cooldown_after_failures`, `cooldown_seconds`).
- **Rate limiting and lockout** enforced by the broker, per peer UID and per
  username.
- **Security-hardening roadmap** in `docs/security-issues/00`–`09` covering
  broker isolation, runtime hardening, liveness PAD, TPM sealing, stricter
  recognition defaults, and dual-camera cross-check (v0.3.0).

### Changed

- **IPC protocol bumped to v2.** Clients built against v1 are rejected with
  `VersionMismatch`. Reinstall both the broker and the CLI together.
- **Legacy `Match` endpoint restricted to `uid=0`.** Non-root callers must
  use `MatchFrame`. This closes the synthetic-embedding bypass available to
  any same-UID process under v1.
- **CLI `auth` and `watch` paths no longer load SCRFD or ArcFace.** They
  open the camera, capture a frame, and submit it to the broker. The
  detector + embedder live exclusively inside `facegate-brokerd`.
- **Template ownership moves from the enrolled user to the `facegate`
  system user.** Stored embeddings are no longer readable by the
  authenticated user's own processes.
- **`FrameProbe.bytes` is base64-encoded** in the JSON envelope, avoiding
  the 4× bloat of integer-array serialisation. Max request size 12 MB,
  covers 1080p RGB.
- **PAM helper subprocess timeout reduced to 25 s** (was 45 s) — password
  fallback feels less sluggish after a missed face.
- Face auth failure messages clarified to distinguish "not recognised",
  "timeout", "camera error", and "configuration error".

### Security

- Templates can no longer be read by the enrolled user's own processes
  (broker-owned storage under `/var/lib/facegate/users`, mode `0600`,
  uid/gid `facegate:facegate`).
- A same-UID attacker can no longer authenticate by replaying a captured
  embedding through `Match` — `MatchFrame` is the only path for non-root
  callers and requires a real camera frame.
- Probe embeddings are zeroised after each match; loaded templates are
  zeroised after each comparison.
- `MatchFrame` rejects frames whose declared geometry exceeds 4096² or
  whose buffer length disagrees with `width × height × bytes-per-pixel`.

### Not yet closed

- **Liveness / presentation attack detection.** A high-quality photograph
  or replay can still match a single-camera capture. Tracked as
  `docs/security-issues/06-liveness-pad.md` (planned for the next minor).
- **TPM sealing of templates at rest.** Tracked as
  `docs/security-issues/07-tpm-sealing.md`.
- **Dual-stream RGB+IR cross-check** for laptops with both sensors.
  Tracked as `docs/security-issues/09-dual-camera-cross-check.md` and
  GitHub issue #28, planned for **v0.3.0**.

## [0.1.0] — 2026-05-10

Initial public release.

### Added

- Face authentication via PAM (`pam_facegate.so`) for `sudo`, `login`,
  and other PAM-aware services.
- Screen-unlock daemon `facegate watch` listening to `org.freedesktop.login1`
  Lock/Unlock signals on the system bus.
- V4L2 capture supporting MJPEG, YUYV, and GREY (IR mono) formats.
- SCRFD face detection and ArcFace embedding via ONNX Runtime.
- Cosine-similarity matching against on-disk templates.
- Interactive TUI for enrolment, listing, and removal of templates.
- Multi-distro packaging (`.deb`, `.rpm`, `.pkg.tar.zst`) via nFPM, with
  GPG-signed packages when a release key is configured.
