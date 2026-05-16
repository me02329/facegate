# Changelog

All notable changes to Facegate are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
on a best-effort basis while the IPC protocol stabilises.

## [Unreleased]

### Changed

- **Default face models swapped for licence cleanup.** v0.3.x downloaded
  the InsightFace `buffalo_l.zip` bundle, whose pre-trained models are
  released for non-commercial research use only even though the
  surrounding code is MIT. v0.4.0 replaces both defaults with
  permissively-licensed alternatives:
  - Face detection: **OpenCV YuNet** (`face_detection_yunet_2023mar.onnx`,
    MIT, ~233 KB). Anchor-free, decoder re-implemented in
    `facegate_core::detection` to match
    `opencv/objdetect/face_detect.cpp`. Landmarks remapped from YuNet's
    `[right_eye, left_eye, nose, right_mouth, left_mouth]` order to the
    `[left_eye, right_eye, nose, left_mouth, right_mouth]` order
    ArcFace alignment expects.
  - Face recognition: **AuraFace v1** (`glintr100.onnx`, Apache-2.0,
    ResNet-100, ~261 MB) from `fal/AuraFace-v1` on HuggingFace.
    Drop-in compatible with the existing `ArcFaceEmbedder::extract`
    pipeline — same 112×112 RGB input, same 512-d L2-normalised output.
  Install scripts (`install-dev.sh`, `packaging/nfpm/scripts/postinstall.sh`)
  now fetch the two ONNX files directly from their authoritative
  HuggingFace mirrors instead of unzipping a bundle, with the YuNet
  SHA256 pinned and a `TBD-pin-before-release` sentinel on the AuraFace
  SHA that hard-fails until a release engineer pins the real value. (#52)
- **Breaking: existing users must re-enrol.** The two embedders sit in
  different 512-d latent spaces, so templates produced by the v0.3.x
  ArcFace will never match against captures embedded by AuraFace.
  `facegate doctor` detects this case (any user whose
  `embeddings.json` predates the configured embedder model file) and
  lists the affected users with the `facegate add` command needed.
  The migration nudge is informational — the broker still loads and
  password fallback keeps working — so upgrades do not lock anyone
  out. (#52)

### Fixed

- PAM helper no longer cuts off legitimate sudo + cross-check auth runs.
  The 25 s hard timeout in `pam_facegate.so` was sized for the RGB-only
  defaults and broke under `[recognition.sudo].max_attempts = 5` with
  `[camera.cross_check].enabled = true`, where the inner capture-retry
  loop legitimately needs up to ~75 s. `facegate auth` now derives its
  own deadline from the effective recognition policy and camera config
  and bails cleanly with `AuthExitCode::Timeout`; the PAM module's hard
  cap is demoted to a 180 s "helper hung" safety net. `facegate doctor`
  surfaces the worst-case sudo and session waits. (#53)
- `match_frame_pair` requests landing on a broker that does not require
  cross-check are now refused with `bad_request` instead of silently
  downgrading to a single-RGB match; the previous behaviour hid the
  client/broker config mismatch, wasted the IR capture, and mislabelled
  the audit event. (#53)
- ArcFace face alignment now uses bilinear sampling instead of
  nearest-neighbour, recovering a few percent of cosine similarity on
  tilted faces and matching the resampling the model was trained
  against. (#53)
- Removed a dead 112×112-to-112×112 resize that allocated a buffer copy
  on every embedding call. (#53)

### Documentation

- `FrameProbe::captured_at_ms` now documents the `0` sentinel
  ("not provided") explicitly in both rustdoc and
  `docs/ipc-protocol.md`. (#53)

## [0.3.1] — 2026-05-15

Distribution-only release: no runtime or behavioural changes. Hardens
the release pipeline so downstream packagers (AUR, COPR, OBS, mirrors)
can verify what they pull and reproduce the artifact set.

### Added

- Release pipeline now produces a source tarball (`facegate-<ver>.tar.gz`)
  and a CycloneDX SBOM (`facegate-<ver>.cdx.json`) alongside the
  `.deb` / `.rpm` / `.pkg.tar.zst` archives. Both are included in
  `checksums.sha256` and covered by the build provenance attestation.
- Sigstore keyless build provenance attestation for every release
  artifact via `actions/attest-build-provenance`. Consumers verify with
  `gh attestation verify <file> --repo me02329/facegate`.
- Release body is now auto-extracted from the matching `## [X.Y.Z]`
  section in `CHANGELOG.md`; releases fail fast if the section is
  missing.
- Dependabot tracks GitHub Actions and Cargo dependencies weekly,
  targeting `dev`.
- README *Verify before installing* section documenting the
  `sha256sum -c` and `gh attestation verify` flows.

### Changed

- All third-party GitHub Actions are now pinned to immutable commit
  SHAs with a tag-version comment, eliminating the previous
  `dtolnay/rust-toolchain@master` mutable ref.

## [0.3.0] — 2026-05-14

This release focuses on operator tooling (broker subcommands,
emergency disable, admin user list), per-scope recognition policy
as a first step toward full per-context auth (#8), an RGB+IR
cross-check used as a liveness *signal* (not a full PAD model —
that work continues in #25), and the contributor / security
disclosure documentation set (`CONTRIBUTING.md`, `SECURITY.md`,
`docs/ipc-protocol.md`, `docs/threat-model.md`,
`docs/recovery.md`).

### Added

- `facegate emergency-disable [--dry-run]` restores clean Facegate PAM
  backups where possible, strips remaining `pam_facegate.so` lines, stops
  the broker/watch services, and is exposed in the TUI behind a
  confirmation prompt. `docs/recovery.md` documents shell, TTY, chroot,
  and live-USB recovery flows (#34, #37).
- `facegate broker` subcommands: `status`, `health`, `restart`, `logs`,
  and `repair-permissions`. The TUI now exposes a top-level Broker entry
  for service/socket/audit/storage status (#33).
- The TUI main menu now exposes more CLI surfaces directly: status, user
  logs, broker health/restart/logs/repair-permissions, and bulk
  per-user template deletion with confirmation. Threshold calibration is
  also available from the TUI with sudo/session scope selection and
  sample-count input.
- `facegate users [--json]` lists enrolled users through the broker,
  including template counts, scopes, enrollment timestamps, and
  directory/file ownership state. The TUI exposes this as Enrolled Users
  (#35).
- `docs/ipc-protocol.md`, `docs/threat-model.md`, and `CONTRIBUTING.md`
  document the broker IPC, security model, and contributor workflow
  (#36, #38, #39).
- Scope-specific recognition policy via `[recognition.sudo]` and
  `[recognition.session]`, with stricter sudo defaults
  (`threshold=0.60`, `required_matches=2`, `max_attempts=5`) while keeping
  session unlock convenience defaults. Calibration writes the selected
  scope's threshold override (#27, first step toward #8).
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
- Optional RGB+IR dual-stream cross-check for auth/watch paths. When
  `[camera.cross_check].enabled = true` and a `[camera.ir]` section is
  set, clients submit a synchronized `MatchFramePair`; the broker
  rejects probes whose capture timestamps disagree, whose RGB or IR
  streams do not contain exactly one face, or whose mapped landmark
  positions are too far apart. The IR stream is used as a **liveness
  signal** (face presence + spatial alignment), not for cross-modal
  identity matching — ArcFace is trained on RGB faces and produces
  meaningless similarities against IR crops, which would otherwise
  reject every genuine user and fail harder in low light (#28).
- `facegate calibrate-cameras`, which captures RGB+IR landmark pairs
  in parallel scoped threads, estimates the IR→RGB homography, reports
  reprojection error, and can write the cross-check calibration back to
  the config (#28).
- Camera frames now carry their `captured_at_ms` timestamp stamped
  inside `V4lCamera::capture_frame` (right after `stream.next()`)
  rather than at IPC submission time, so the broker's
  `max_time_skew_ms` window measures real RGB↔IR capture skew.
- Dedicated `[camera.ir]` config section with per-IR overrides for
  `width`, `height`, `fps`, `timeout_ms`, `warmup_frames`, and
  `min_face_size`, all optional with IR-friendly defaults (longer
  warmup/timeout, 5/8× the RGB min face size).
- `facegate forget <username> [--yes]` removes every enrolled template
  for a user in one go, with confirmation prompt by default.
- Per-user diagnostic log at `~/.local/state/facegate/facegate.log`, plus
  `facegate logs`, to help users debug camera failures, timeouts,
  cross-check rejects, broker errors, and accept/reject outcomes without
  reading root-owned system logs.
- Automatic service refresh after config writes: `configure`, `setup`,
  `calibrate --write`, and `calibrate-cameras --write` now start/restart
  `facegate-brokerd.service` and restart the user's `facegate-watch`
  service if it is active, so config changes apply immediately.
- TUI idle pane now shows a live system status block — broker socket
  reachability, watch service activity, sudo / session PAM, RGB and
  IR camera presence, last audit event (outcome and age), and
  enrolled template count — pushed every 1.5 s by a background probe
  thread so the indicators refresh without user input. Long command
  output (logs, status, audit entries) now wraps inside the right
  pane instead of clipping past the border (closes #47).

### Changed

- `facegate status` from the TUI now matches the CLI output: the
  camera, models, templates, and audit sections were previously only
  printed by the CLI variant. Audit event lines also use lower-case
  scope / outcome / reason labels instead of the Rust `Debug`
  representation (#47).
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
- **IPC protocol bumped to v5** (was v3 in the previous Unreleased
  drop): `MatchFramePair` carries server-meaningful per-frame capture
  timestamps and the `cross_check_identity_mismatch` reason is removed
  (the broker no longer runs ArcFace on the IR crop). `min_identity_similarity`
  is removed from `[camera.cross_check]`; `camera.ir_device` (string) is
  replaced by a dedicated `[camera.ir]` section. Cross-check enabled with
  the identity homography is refused at config validation unless
  `camera.cross_check.allow_identity_homography = true` (#28).
- **Default `max_time_skew_ms` bumped 50 → 200 ms.** The 50 ms window
  was too tight for typical Chicony/Realtek IR modules, whose first
  frame after `STREAMON` is regularly 80–150 ms slower than the RGB
  one. 200 ms keeps the window short enough to bound replay risk
  while letting honest dual-camera captures through on the first
  attempt (#28).
- `install-dev.sh` next-steps message now points operators at
  `sudo facegate setup` (which picks RGB as primary and offers
  cross-check + calibration inline) instead of telling them to prefer
  the IR device as the primary camera (which broke validation).

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
