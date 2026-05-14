# Facegate

<p align="center">
  <strong>Native Rust facial authentication for Linux PAM</strong>
</p>

<p align="center">
  Face authentication for <code>sudo</code>, login sessions, and screen unlock — fully local, scriptable, and designed to avoid the legacy Python/PAM stack.
</p>

<p align="center">
  <a href="https://github.com/me02329/facegate">
    <img alt="Language" src="https://img.shields.io/badge/language-Rust-orange">
  </a>
  <a href="https://github.com/me02329/facegate">
    <img alt="Linux" src="https://img.shields.io/badge/platform-Linux-blue">
  </a>
  <a href="https://github.com/me02329/facegate">
    <img alt="PAM" src="https://img.shields.io/badge/auth-PAM-informational">
  </a>
  <a href="https://github.com/me02329/facegate">
    <img alt="On device" src="https://img.shields.io/badge/ML-on--device-success">
  </a>
  <a href="https://github.com/me02329/facegate/blob/master/LICENSE">
    <img alt="License" src="https://img.shields.io/github/license/me02329/facegate">
  </a>
</p>

---

## What is Facegate?

Facegate is a native Linux facial authentication stack written in Rust.

It provides a standard PAM module, an interactive terminal UI, a background screen-unlock daemon, and — since **v0.2.0** — a dedicated system broker daemon (`facegate-brokerd`) that owns all biometric templates and runs the ML matching pipeline in isolation. It is designed for Linux laptops with RGB or IR cameras and runs the full recognition pipeline locally using ONNX Runtime.

Unlike legacy tools that depend on Python, `pam-python`, Python 2, or fragile dlib builds, Facegate keeps the PAM integration small and auditable:


**Native facial authentication for Linux — including automatic screen unlock.**

Facegate lets you authenticate with your face for `sudo`, login sessions, and screen lock. It runs entirely on-device: no cloud, no network, no telemetry. The ML pipeline (SCRFD face detection + ArcFace embeddings) runs locally via ONNX Runtime, inside a dedicated, sandboxed system daemon.


---
<img width="1615" height="970" alt="image" src="https://github.com/user-attachments/assets/6f40bec9-b786-41cf-8805-edf4e9a01a1f" />

---

## Features

- **Automatic screen unlock** — a background daemon watches for the lock signal via D-Bus and unlocks the screen as soon as your face is recognised, with no keypress required (Windows Hello style)
- Face authentication via a standard Linux PAM module (`pam_facegate.so`) for `sudo`, `su`, and login managers (SDDM, LightDM, GDM, greetd)
- **Privileged broker daemon (`facegate-brokerd`)** — a dedicated system service running as the unprivileged `facegate` user that owns templates, runs SCRFD + ArcFace, and exposes only match decisions over a local Unix socket (added in v0.2.0)
- **Frame-based matching (`MatchFrame`)** — clients submit raw camera frames; the broker performs detection, embedding extraction, and comparison itself. A same-UID attacker cannot bypass live capture by replaying a precomputed embedding (v0.2.0)
- Interactive TUI to configure, enroll faces, run diagnostics, and manage all auth modes
- Guided first-time setup flow (`facegate setup`) covering camera selection, enrolment, and PAM wiring
- Threshold calibration command (`facegate calibrate`) that recommends a recognition threshold from live positive samples
- Compact installation summary (`facegate status`) including broker reachability, model presence, enrolled templates, and recent audit events
- Multi-sample enrollment with separate templates per capture for better accuracy
- Per-template auth scopes (`sudo`, `session`, or `both`)
- ArcFace embeddings + SCRFD face detector (ONNX Runtime, fully on-device)
- **Privacy-preserving audit log** at `/var/lib/facegate/audit.log` — coarse outcome/reason only; no images, no embeddings, no similarity scores
- Configurable cooldown / lockout after repeated failed matches, enforced server-side by the broker
- Password fallback configurable per PAM service
- Shell completions for bash, zsh, fish

---
<img width="878" height="224" alt="image" src="https://github.com/user-attachments/assets/9faee7ed-22a7-4f32-b85a-95aad87dd99e" />


---

## Requirements

- Linux with a V4L2 camera (`/dev/video*`)
- Root access for installation and enrollment
- systemd (for both the screen-lock watch daemon **and** the broker daemon)

Everything else (ONNX Runtime, face recognition models) is downloaded automatically by the install script.

### Choosing the right camera

Most laptops with Windows-Hello-style hardware expose **two** capture
devices: a regular RGB webcam (typically `/dev/video0`) and a separate
IR / depth sensor (often `/dev/video2`, sometimes `/dev/video4`). On
desktops you usually only have a USB webcam and that's `/dev/video0`.

Both work, but the **IR camera is strongly recommended**:

- it works in the dark — your screen-unlock keeps working at night;
- it ignores the visible screen reflection on your face;
- it's significantly harder to spoof with a printed photograph than an
  RGB feed.

To find which device is which:

```bash
facegate cameras            # built-in: lists devices, flags IR vs RGB,
                            # and recommends the best one
v4l2-ctl --list-devices     # vendor names
```

Devices that report `GREY` / `Y8` / `Y800` formats are IR streams; devices
that only report `YUYV` / `MJPG` are RGB. Update `[camera].device` in
`/etc/facegate/config.toml` accordingly (or run `sudo facegate configure`).

> **Dual-camera cross-check.** On laptops that expose both an IR and an
> RGB sensor, Facegate can require a synchronized match on *both* streams
> to reduce single-camera photo/replay attacks.
> Tracked as `docs/security-issues/09-dual-camera-cross-check.md`.

---

## Installation

### From GitHub Releases (recommended)

Download the package for your distribution from the latest GitHub Release:

- Arch Linux: `facegate-<version>-1-x86_64.pkg.tar.zst`
- Debian / Ubuntu: `facegate_<version>_amd64.deb`
- Fedora / openSUSE / RPM-based: `facegate-<version>.x86_64.rpm`

```bash
# Arch Linux
sudo pacman -U ./facegate-<version>-1-x86_64.pkg.tar.zst

# Debian / Ubuntu
sudo apt install ./facegate_<version>_amd64.deb

# Fedora
sudo dnf install ./facegate-<version>.x86_64.rpm

# openSUSE
sudo zypper install ./facegate-<version>.x86_64.rpm
```

After installation:

```bash
sudo facegate doctor        # verify everything is in place
sudo facegate status        # broker reachability + enrolment summary
sudo facegate camera-test   # confirm the camera works
```

The packaged install creates:

- `/usr/bin/facegate` — CLI / TUI / watch daemon
- `/usr/bin/facegate-brokerd` — system broker daemon (new in v0.2.0)
- `/usr/lib/security/pam_facegate.so`
- `/usr/lib/systemd/system/facegate-brokerd.service` (new in v0.2.0)
- `/usr/lib/systemd/user/facegate-watch.service`
- `/etc/facegate/config.toml`
- `/usr/share/facegate/models/`
- `/var/lib/facegate/users/` — owned by the `facegate` system user, mode `0700`
- `/var/lib/facegate/audit.log` — owned by `facegate:facegate`, mode `0600` (new in v0.2.0)

The postinstall script also creates a dedicated **`facegate` system user
and group**, migrates any pre-existing template directories from the
enrolled user's ownership to `facegate:facegate`, and enables
`facegate-brokerd.service` automatically.

### Development Install

```bash
# 1. Build (as your normal user)
cargo build --release

# 2. Install (as root)
sudo bash install-dev.sh
```

The install script copies the binary (`facegate` **and** `facegate-brokerd`), PAM module, both systemd units, config, man page, and shell completions, and downloads ONNX Runtime and face models if not already present. Use `--skip-ort` or `--skip-models` to skip downloads.

`install-dev.sh` does not yet create the `facegate` system user or
auto-start the broker — use the packaged install (or run the
postinstall script manually) if you want the v0.2.0 hardened storage
layout.

### Building Packages

```bash
FACEGATE_VERSION=0.2.0 scripts/package-nfpm.sh
```

This produces `.deb`, `.rpm`, and `.pkg.tar.zst` packages in `dist/`.

---

## Quick Start

```bash
# 0. Find the IR (or fallback RGB) camera and update the config (no root):
facegate cameras
sudo facegate configure                      # set [camera].device

# 1. One-shot guided flow (recommended for new installs):
sudo facegate setup                          # camera → enrolment → PAM wiring

# Or open the interactive menu:
sudo facegate

# Or step by step:
sudo facegate doctor                         # verify installation
sudo facegate status                         # broker + enrolment summary
sudo facegate add $USER --for both           # enroll your face
sudo facegate test $USER                     # verify recognition
sudo facegate calibrate $USER --write        # tune the threshold from real samples
sudo facegate session-auth                   # enable PAM for login & screen lock
systemctl --user enable --now facegate-watch # start the auto-unlock daemon
```

The system broker (`facegate-brokerd.service`) is enabled automatically
by the package postinstall; you should not normally need to start it
yourself. `facegate status` will tell you whether it is reachable.

---

## How Screen Unlock Works

Since v0.2.0, all face-matching decisions go through the broker. The
client side opens the camera, captures a frame, and submits the raw
frame to `facegate-brokerd` over a Unix socket; the broker runs SCRFD +
ArcFace and returns a yes/no decision.

### 1. PAM module — for sudo and login managers

```
sudo / SDDM / LightDM / GDM
  └─→ PAM
       └─→ pam_facegate.so
            └─→ facegate auth --user <name>           (subprocess)
                 └─→ opens camera, captures a frame
                      └─→ MatchFrame over /run/facegate/broker.sock
                           └─→ facegate-brokerd (system daemon)
                                └─→ SCRFD + ArcFace + match
```

The PAM module is called when the user initiates authentication — for example, when running `sudo` or pressing Login at the SDDM screen. If the face matches, PAM succeeds immediately; otherwise it falls through to the next module (password, if fallback is enabled).

The PAM module spawns `facegate auth` as a separate subprocess so the module itself carries no ML, async, or IPC dependencies and remains small and auditable. Since v0.2.0 the subprocess does **not** load SCRFD or ArcFace either — it only captures a frame and hands it to the broker.

The PAM helper subprocess timeout is **25 seconds** in v0.2.0 (down
from 45 s) so password fallback feels less sluggish after a missed
face.

### 2. Watch daemon — for screen lock (Windows Hello style)

```
logind
  └─→ Lock signal (D-Bus)
       └─→ facegate-watch (user daemon)
            └─→ opens camera via logind session ACLs
            └─→ captures a frame
                 └─→ MatchFrame → facegate-brokerd
                      └─→ face recognised → loginctl unlock-session
```

`facegate-watch` is a systemd user service that subscribes to `org.freedesktop.login1.Session.Lock` on the system D-Bus. The moment the screen locks, it opens the camera and starts capturing frames. Each frame is submitted to the broker, and as soon as the broker returns a match, `facegate-watch` calls `org.freedesktop.login1.Session.Unlock()` directly — no keypress needed.

If recognition fails or times out, the daemon stops the camera and lets the user type their password normally. If the user types their password first (the `Unlock` signal fires), any ongoing scan is cancelled immediately.

This gives a Windows-Hello-style unlock experience: a dedicated process reacts to a system event, the user never needs to interact with an unlock form, and the camera is released as soon as a decision is made. Since v0.2.0, biometric matching also runs out-of-process from the user's session, behind systemd hardening — see [Security](#security).

---

## Enrollment

Templates are scoped to their authentication target.

```bash
sudo facegate add $USER --for sudo     # for sudo / su only
sudo facegate add $USER --for session  # for login manager + screen lock
sudo facegate add $USER --for both     # for all flows (recommended for most users)
```

Sudo-scoped templates are rejected for session auth and vice versa. `--for both` covers all flows with a single enrollment.

Facegate prompts for the number of samples to capture (1–10, default 3). Each sample is stored as a separate template, improving accuracy across varying poses and lighting.

Since v0.2.0, enrollment goes through the broker: the CLI captures
frames, sends them to `facegate-brokerd`, and the broker is the only
process that writes to `/var/lib/facegate/users/`. The on-disk
ownership is `facegate:facegate` (mode `0600`) regardless of which
user the templates belong to — they are no longer `chown`ed to the
enrolled user.

```
$ sudo facegate add mart --for both
How many samples do you want to capture? [1-10, default 3]: 3

Enrolling sudo+session face for 'mart' (label: 'mart', 3 sample(s))
Opening camera and contacting facegate-brokerd...

Sample 1/3 — position yourself in front of the camera, then press Enter...
Capturing (timeout: 5000ms)...
  ✓ template #0 saved (label: 'mart-1')
...
Done — 3 template(s) enrolled for 'mart'.
```

---

## PAM Setup

### Via the TUI (recommended)

Run `sudo facegate` and use the interactive menu:

- **Sudo Auth** — toggle `pam_facegate.so` in `/etc/pam.d/sudo` and (when present) `/etc/pam.d/sudo-i`
- **Session Auth** — toggle `pam_facegate.so` in detected login/session PAM services (SDDM, LightDM, GDM, greetd, `login`, `kde`, …)
- **Watch Daemon** — enable/disable `facegate-watch.service` for automatic screen unlock

### Manual setup

Add the following line **before** the existing `auth` lines in any PAM service file:

```
auth      sufficient    /usr/lib/security/pam_facegate.so
```

The absolute path is intentional — it makes the line work on every distro,
including Debian/Ubuntu (which search `/usr/lib/x86_64-linux-gnu/security`)
and Fedora (`/usr/lib64/security`). The bare-name form
(`pam_facegate.so`) only works on distros whose PAM search path matches our
install dir, so we no longer recommend it.

> **Warning:** Always keep a root shell open while editing PAM configuration. A broken PAM config can lock you out of `sudo` and login.

Enable the watch daemon as your normal user:

```bash
systemctl --user enable --now facegate-watch
```

### Supported session PAM services

`session-auth` auto-detects: `login`, `gdm-password`, `gdm3`, `gdm`, `sddm`, `lightdm`, `greetd`, `kde`, `gnome-screensaver`, `swaylock`, `hyprlock`, `i3lock`, `vlock`.

---

## CLI Reference

| Command | Description |
|---|---|
| *(none)* | Open the interactive TUI menu |
| `configure` | Edit settings in a terminal UI |
| `setup [USERNAME]` | Guided first-time setup flow (camera → enrol → PAM wiring) |
| `status` | Compact installation, broker reachability, and enrolment summary (also shows recent audit events) |
| `logs [--lines N]` | Show the current user's local diagnostic log |
| `doctor` | Check installation status |
| `cameras` | List `/dev/video*` and flag IR vs RGB |
| `camera-test [--device DEV]` | Test camera and face detection |
| `add USERNAME [--label LABEL] [--for sudo\|session\|both]` | Enroll face templates |
| `list USERNAME` | List enrolled templates (via the broker) |
| `remove USERNAME ID` | Remove a template by ID (via the broker) |
| `test USERNAME [--for sudo\|session\|all]` | Live recognition test |
| `calibrate USERNAME [--for sudo\|session] [--samples N] [--write]` | Recommend a recognition threshold from live positive samples; `--write` offers to save it to the config |
| `calibrate-cameras [--rgb-device DEV] [--ir-device DEV] [--samples N] [--write] [--enable]` | Estimate the IR→RGB homography for dual-stream cross-check |
| `session-auth` | Toggle face auth in login/session PAM services |
| `completions SHELL` | Print shell completion script |

All commands except `completions`, `cameras`, `status`, and the internal
`watch`/`auth` helpers require root. `cameras`, `status`, `logs`, and `watch` run as
the normal user. All template reads/writes and all match decisions
ultimately go through `facegate-brokerd` over `/run/facegate/broker.sock`.

Facegate also writes a user-readable diagnostic log at
`~/.local/state/facegate/facegate.log`. It records coarse local events such as
camera errors, timeouts, cross-check rejects, match scores, and accept/reject
outcomes. It does not contain frames or embeddings. Use `facegate logs` to view
recent lines.

---

## Configuration

`/etc/facegate/config.toml` — edit with `sudo facegate configure` or directly.

```toml
[camera]
device = "/dev/video0"
width = 640
height = 480
fps = 30
timeout_ms = 5000
warmup_frames = 5

# [camera.ir]
# device = "/dev/video2"      # IR sensor for RGB+IR cross-check
# # All other fields optional; blank = IR-friendly defaults
# # min_face_size = 50

[camera.cross_check]
enabled = false
max_time_skew_ms = 50
max_position_offset_px = 40.0
homography = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
allow_identity_homography = false   # refuse identity matrix unless explicit

[recognition]
threshold = 0.55        # cosine similarity threshold (higher = stricter)
required_matches = 1    # how many templates must match
max_attempts = 5        # capture attempts before giving up
min_face_size = 80      # minimum face bounding-box size in pixels

[models]
detector = "/usr/share/facegate/models/scrfd_500m.onnx"
embedder = "/usr/share/facegate/models/arcface_w600k_r50.onnx"

[storage]
base_dir = "/var/lib/facegate/users"

[security]
allow_password_fallback = true
deny_on_camera_error = false
cooldown_after_failures = 10   # broker-enforced lockout threshold
cooldown_seconds = 60          # lockout duration

[logging]
level = "warn"
log_failed_attempts = true
```

The `[security].cooldown_after_failures` / `cooldown_seconds` knobs are
enforced server-side by the broker, per-peer-UID and per-username, so a
hostile client cannot bypass the lockout by reconnecting.

When `[camera.cross_check].enabled = true`, a `[camera.ir]` section must be
configured. Auth clients capture the RGB and IR streams in parallel and
submit a `MatchFramePair` to the broker. The broker rejects the probe unless
the pair is timestamp-synchronized within `max_time_skew_ms`, each stream
contains exactly one face, and the IR face maps via `homography` to within
`max_position_offset_px` of the RGB face. The IR check is a **liveness**
signal (proves a real face is present and aligned); identity matching is done
on the RGB embedding alone. Single-camera systems should leave this disabled.

Use `sudo facegate calibrate-cameras --ir-device /dev/video2 --write` to
estimate and write the `homography` from live RGB+IR landmark pairs. Add
`--enable` only after testing the resulting config with `sudo facegate test
<USER>`.

Whenever Facegate writes `/etc/facegate/config.toml` through `configure`,
`setup`, `calibrate --write`, or `calibrate-cameras --write`, it refreshes
services automatically: the broker is restarted/started so it reads the new
thresholds, model paths, storage path, and cross-check policy, and the
per-user `facegate-watch` daemon is restarted if it is currently active.

You can also use `facegate calibrate <USER> --write` to compute a
threshold from real positive samples rather than guessing at
`[recognition].threshold` by hand.

---

## Architecture

```
facegate/
├── crates/
│   ├── facegate_core/      # camera, detection, embedding, matching, storage, config
│   ├── facegate_ipc/       # versioned JSON-over-Unix-socket protocol (v3)
│   ├── facegate_brokerd/   # privileged broker daemon (facegate-brokerd)
│   ├── facegate_cli/       # CLI + TUI + watch daemon (facegate binary)
│   └── pam_facegate/       # PAM module (pam_facegate.so)
├── packaging/
│   └── nfpm/               # .deb / .rpm / .pkg.tar.zst manifests + postinstall
├── scripts/
│   └── package-nfpm.sh     # one-shot multi-distro package builder
├── systemd/
│   ├── facegate-brokerd.service   # system service (root → drops to facegate user)
│   └── facegate-watch.service     # user service (per-session unlock)
├── docs/
│   ├── facegate.1
│   └── security-issues/    # security hardening roadmap (00–09)
└── install-dev.sh
```

**`facegate_core`** handles the full ML pipeline: V4L2 capture, SCRFD face detection, ArcFace embedding extraction, cosine similarity matching, and secure template storage. Since v0.2.0 the detector + embedder are linked **only** into `facegate-brokerd`; the CLI and PAM helper rely on `facegate_core` exclusively for the V4L2 capture side.

**`facegate_ipc`** defines the versioned JSON-over-Unix-socket protocol between clients and the broker. Protocol version is **v3**; mismatched clients are rejected with `VersionMismatch`. Reinstall both the broker and the CLI together when upgrading.

**`facegate_brokerd`** is the new privileged broker (a system service started by systemd). It runs as the dedicated `facegate` user, owns all enrolled templates, performs face detection / embedding / matching on submitted frames, enforces rate limiting and lockouts, and writes the audit log. It does **not** open camera devices itself — frames arrive over the IPC socket.

**`facegate_cli`** provides the `facegate` binary with a Clap CLI and a Ratatui TUI. It also implements the `watch` subcommand — the D-Bus daemon used by the systemd user service. Since v0.2.0, both `auth` and `watch` capture a frame and submit it to the broker via `MatchFrame`; they no longer load SCRFD or ArcFace.

**`pam_facegate`** is a minimal cdylib that implements `pam_sm_authenticate`. It spawns `facegate auth` as a subprocess so the PAM module itself carries no ML, no async runtime, and no IPC code, making it small and auditable.

---

## Security

### What Facegate is

Facegate is a **convenience authentication mechanism**. It is designed to reduce friction for everyday operations — unlocking your screen, running `sudo` — without replacing the security of a strong password. Password authentication remains available at all times (unless you explicitly disable fallback).

### What it is not

- It is not a replacement for hardware-backed authentication (TPM, FIDO2, smartcard).
- It does not yet implement liveness detection. A high-quality photograph or replay of the enrolled user could theoretically fool a single-camera capture depending on the model and threshold. For high-security scenarios, keep face auth as a first factor and require password confirmation for sensitive operations. Tracked as `docs/security-issues/06-liveness-pad.md` and `09-dual-camera-cross-check.md`.

### No network, no cloud

All processing happens on your machine. Face embeddings are computed locally by ONNX Runtime, inside the broker process. No image, embedding, or identity data is ever sent over the network. There is no telemetry. The broker systemd unit explicitly sets `PrivateNetwork=yes`, `RestrictAddressFamilies=AF_UNIX`, and `IPAddressDeny=any`.

### Trust boundary: the broker

Since **v0.2.0**, the authentication trust boundary lives inside
`facegate-brokerd`, a dedicated system daemon. Clients (PAM helper,
watch daemon, CLI) never read enrolled templates and never decide
match outcomes; they only capture frames and submit them.

```
client (any UID)          ──MatchFrame(raw frame)──▶   facegate-brokerd (uid=facegate)
                          ◀────── MatchResult ─────                    │
                                                              SCRFD + ArcFace
                                                              compare with templates
                                                              owned by facegate:facegate
```

- **`MatchFrame` is the only matching path for non-root callers.** The
  legacy `Match` endpoint (probe-embedding in, decision out) is now
  restricted to `uid=0` — this closes the synthetic-embedding bypass
  available to any same-UID process under v1.
- **The CLI `auth` and `watch` subprocesses no longer load SCRFD or
  ArcFace.** They open the camera, capture a frame, and call the
  broker. Same-UID code execution can no longer fabricate a "this is
  me" embedding without going through real camera capture.
- **Probe embeddings and decoded frame bytes are zeroised** after each
  match; loaded templates are zeroised after each comparison.
- **Frame envelopes are sanity-checked**: `MatchFrame` rejects frames
  whose declared geometry exceeds 4096² or whose buffer length
  disagrees with `width × height × bytes-per-pixel`. The request size
  cap is 12 MB (enough for 1080p RGB after base64).
- **Peer credentials are enforced via `SO_PEERCRED`**, so the broker
  knows which UID owns each connection and can apply per-UID and
  per-username rate limits / lockouts.
- **IPC protocol is versioned (v3).** A mismatched client is rejected
  with `VersionMismatch`; you must reinstall the CLI and the broker
  together.

### Template storage

Face templates are stored as ArcFace embedding vectors — compact biometric templates derived from face images. They are not photographs, but they are sensitive biometric data. Published model-inversion and template-inversion techniques can sometimes produce face-like images or transferable biometric artifacts from embeddings, so Facegate treats templates as secrets.

- Templates are stored under `/var/lib/facegate/users/<username>/embeddings.json`
- Ownership: **`facegate:facegate`** (the dedicated system user), mode `0600`
- The enclosing per-user directory is mode `0700`, also owned by `facegate:facegate`
- Writes are performed exclusively by the broker, atomically (write to `.tmp`, `fsync`, `rename`) — no partial state on power loss
- Symlink traversal is blocked on all file operations
- The package postinstall migrates pre-existing template directories from the enrolled user's ownership to `facegate:facegate`

**Same-UID exfiltration is no longer trivial.** Under v0.1.0 the
template file was owned by the enrolled user, so any process running
as that user could read its own ArcFace vector. As of v0.2.0 the file
is owned by the `facegate` system user (the broker's uid) and is
**not readable by the enrolled user's own processes**. A normal user
process can only request a match decision via `MatchFrame`; it cannot
read enrolled vectors. This is a meaningful reduction of the
biometric-leak surface — vectors are still not photos, but a
sufficiently capable adversary could use them for model inversion,
transfer attacks, or image-generation workflows, so keeping them
behind the broker matters. Templates at rest are still **not** TPM-
sealed in v0.2.0 — see `docs/security-issues/07-tpm-sealing.md`.

### Audit log

Facegate keeps a local, privacy-preserving audit log at
`/var/lib/facegate/audit.log`, written exclusively by the broker
(uid=`facegate`, mode `0600`). Each record contains a timestamp,
username, auth scope (`sudo`/`session`), coarse outcome
(`success`/`failure`) and coarse reason (`matched`, `mismatch`,
`not_enrolled`, `rate_limited`, `locked_out`, `unauthorized`,
`internal`). It does not log images, embeddings, similarity scores,
or any score-derived data. Writes are best-effort and never required
for authentication to complete. `facegate status` shows recent
events when the broker authorises access.

### Broker hardening (systemd)

`facegate-brokerd.service` is shipped with an aggressive systemd
sandbox:

- runs as the dedicated `facegate` user/group (no caps, no ambient caps)
- `NoNewPrivileges=yes`, `MemoryDenyWriteExecute=yes`, `LockPersonality=yes`
- `RestrictAddressFamilies=AF_UNIX`, `PrivateNetwork=yes`, `IPAddressDeny=any` — local Unix socket only
- `PrivateDevices=yes` — broker never opens `/dev/video*`; only clients do
- `PrivateTmp=yes`, `ProtectSystem=strict`, `ProtectHome=yes`
- `ProtectKernelTunables/Modules/Logs=yes`, `ProtectControlGroups=yes`, `ProtectClock=yes`, `ProtectHostname=yes`
- `ProtectProc=invisible`, `ProcSubset=pid`
- `RestrictNamespaces=yes`, `RestrictRealtime=yes`, `RestrictSUIDSGID=yes`
- `SystemCallFilter=@system-service` minus `@mount @debug @cpu-emulation @obsolete @raw-io @reboot @swap @privileged`
- `LimitCORE=0` — no core dumps that could leak embedding state
- read-write paths restricted to `/run/facegate` (socket) and `/var/lib/facegate` (templates + audit log)

### PAM module

`pam_facegate.so` is deliberately minimal. It does not link ONNX Runtime, does not load models, does not open the camera, and does not speak the broker protocol. It spawns `/usr/bin/facegate auth --user <name>` as a subprocess and interprets its exit code. A **25-second** hard timeout (reduced from 45 s in v0.1.0) ensures PAM is never blocked indefinitely regardless of what happens to the subprocess.

### Watch daemon attack surface

`facegate-watch` has no listening socket of any kind — it cannot be reached over the network or via a local Unix socket. Its only inputs are D-Bus signals from the system bus and the local broker socket it connects out to.

**D-Bus signal authenticity.** The `Lock` and `Unlock` signals the daemon listens to are emitted by `org.freedesktop.login1`, the service name owned exclusively by systemd-logind, which runs as root. An unprivileged process cannot claim that service name on the system bus. A process running as a different user cannot inject fake `Lock` signals — the D-Bus daemon verifies sender credentials using kernel socket credentials (`SO_PEERCRED`). Only systemd-logind can trigger a recognition scan.

**Unlocking.** When a face is recognised by the broker, the daemon calls the `Unlock()` method on the `org.freedesktop.login1.Session` object for the current session. This is authorised by polkit because the caller owns the session (same UID, same session ID). A process in a different session or with a different UID cannot call `Unlock()` on someone else's session.

**Camera access without the `video` group.** The daemon runs as the logged-in user within their active logind session. systemd-logind + udev automatically grant the active session's owner access to `/dev/video*` through filesystem ACLs set at session activation time. The `video` group is not required and is not added. This is the proper Linux session permission model — the same one used by PipeWire, PulseAudio, and other session-aware daemons.

**Same-UID attacker.** If an attacker already has code execution as the same user, they can submit frames to the broker just like the legitimate watch daemon — but they cannot read enrolled templates, fabricate an embedding, or bypass per-UID/per-username rate limiting and lockouts enforced by the broker. They also cannot replay an old embedding via the legacy `Match` endpoint, which is restricted to `uid=0` since v0.2.0. Same-UID compromise is still bad — they have your camera, your D-Bus, and your filesystem — but the biometric template and the match decision are no longer in reach.

### Comparison with Windows Hello

Windows Hello uses a privileged biometric service and hardware-backed protections to keep biometric templates outside normal user processes and to bind matching to live camera frames.

As of **v0.2.0**, Facegate has reached the first of those two
properties:

- a dedicated system daemon (`facegate-brokerd`) owns all templates;
- normal user processes cannot read enrolled vectors;
- matching only happens broker-side, after the broker has seen a real
  camera frame submitted by the client.

What is **not yet** equivalent to Windows Hello:

- **Liveness / presentation attack detection.** A high-quality
  photograph or replay can still match a single-camera capture. The
  broker validates the frame envelope's geometry and size, not the
  semantic authenticity of the pixels. Planned for the next minor
  release (`docs/security-issues/06-liveness-pad.md`).
- **Dual-stream RGB+IR cross-check.** Laptops with both sensors can,
  in the future, be required to match on *both* streams. Tracked as
  `docs/security-issues/09-dual-camera-cross-check.md` and GitHub
  issue #28, planned for **v0.3.0**.
- **TPM sealing of templates at rest.** Templates are protected by
  Unix file permissions but not yet sealed to platform state
  (`docs/security-issues/07-tpm-sealing.md`).
- **Broker-side frame acquisition.** Windows Hello captures frames
  inside its privileged service; Facegate's broker runs
  `PrivateDevices=yes` and instead receives frames from clients. A
  same-UID attacker can therefore *submit* a frame, but the frame
  must still pass detection + matching server-side; liveness PAD is
  the missing piece here.

### Summary

| Property | Sudo auth | Watch daemon |
|---|---|---|
| Runs as | root (via PAM) → broker (`facegate`) | user (session daemon) → broker (`facegate`) |
| Camera access | client side, direct (root) | client side, via logind session ACLs |
| ML pipeline (SCRFD + ArcFace) | broker only (`facegate-brokerd`) | broker only (`facegate-brokerd`) |
| Match decision | broker | broker |
| `video` group needed | no | no |
| Network exposure | none (`PrivateNetwork=yes` on broker) | none |
| D-Bus exposure | none | subscriber only (no listener) |
| IPC exposure | AF_UNIX, `SO_PEERCRED`-checked, v3 protocol | AF_UNIX, `SO_PEERCRED`-checked, v3 protocol |
| Forging a trigger | N/A — user initiates | requires impersonating systemd-logind (impossible for unprivileged code) |
| Template readable by | `facegate` system user only | `facegate` system user only |
| Replayable embedding bypass | blocked — `Match` restricted to uid=0, `MatchFrame` requires a frame | blocked — same as sudo path |
| Rate limit / lockout | broker-enforced, per UID + per username | broker-enforced, per UID + per username |
| Audit log | yes, broker-written, no embeddings/scores | yes, broker-written, no embeddings/scores |

---

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for the full release history. The
current release, **v0.2.0** (2026-05-11), moves the biometric trust
boundary into the `facegate-brokerd` daemon, restricts the legacy
`Match` endpoint to root, adds the privacy-preserving audit log, and
introduces the `setup`, `status`, and `calibrate` commands.

---

## License

Facegate is licensed under the GNU General Public License v3.0 or later.

You are free to use, study, modify, and redistribute the software. Modified
redistributions must preserve the same open-source freedoms under the GPL.
See [LICENSE](LICENSE) for details.
