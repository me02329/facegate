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

It provides a standard PAM module, an interactive terminal UI, and a background screen-unlock daemon. It is designed for Linux laptops with RGB or IR cameras and runs the full recognition pipeline locally using ONNX Runtime.

Unlike legacy tools that depend on Python, `pam-python`, Python 2, or fragile dlib builds, Facegate keeps the PAM integration small and auditable:


**Native facial authentication for Linux — including automatic screen unlock.**

Facegate lets you authenticate with your face for `sudo`, login sessions, and screen lock. It runs entirely on-device: no cloud, no network, no telemetry. The ML pipeline (SCRFD face detection + ArcFace embeddings) runs locally via ONNX Runtime.


---
<img width="1615" height="970" alt="image" src="https://github.com/user-attachments/assets/6f40bec9-b786-41cf-8805-edf4e9a01a1f" />

---

## Features

- **Automatic screen unlock** — a background daemon watches for the lock signal via D-Bus and unlocks the screen as soon as your face is recognised, with no keypress required (Windows Hello style)
- Face authentication via a standard Linux PAM module (`pam_facegate.so`) for `sudo`, `su`, and login managers (SDDM, LightDM, GDM, greetd)
- Interactive TUI to configure, enroll faces, run diagnostics, and manage all auth modes
- Multi-sample enrollment with separate templates per capture for better accuracy
- ArcFace embeddings + SCRFD face detector (ONNX Runtime, fully on-device)
- Template storage scoped per auth target (sudo vs. session)
- Password fallback configurable per PAM service
- Shell completions for bash, zsh, fish

---
<img width="878" height="224" alt="image" src="https://github.com/user-attachments/assets/9faee7ed-22a7-4f32-b85a-95aad87dd99e" />


---

## Requirements

- Linux with a V4L2 camera (`/dev/video*`)
- Root access for installation and enrollment
- systemd (for the screen-lock watch daemon)

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
sudo facegate camera-test   # confirm the camera works
```

The packaged install creates:

- `/usr/bin/facegate`
- `/usr/lib/security/pam_facegate.so`
- `/usr/lib/systemd/user/facegate-watch.service`
- `/etc/facegate/config.toml`
- `/usr/share/facegate/models/`
- `/var/lib/facegate/users/`

### Development Install

```bash
# 1. Build (as your normal user)
cargo build --release

# 2. Install (as root)
sudo bash install-dev.sh
```

The install script copies the binary, PAM module, systemd unit, config, man page, and shell completions, and downloads ONNX Runtime and face models if not already present. Use `--skip-ort` or `--skip-models` to skip downloads.

### Building Packages

```bash
FACEGATE_VERSION=0.1.0 scripts/package-nfpm.sh
```

This produces `.deb` and `.rpm` packages in `dist/`.

---

## Quick Start

```bash
# 0. Find the IR (or fallback RGB) camera and update the config (no root):
facegate cameras
sudo facegate configure                      # set [camera].device

# 1. Open the interactive menu (requires root)
sudo facegate

# Or step by step:
sudo facegate doctor                         # verify installation
sudo facegate add $USER --for both           # enroll your face
sudo facegate test $USER                     # verify recognition
sudo facegate session-auth                   # enable PAM for login & screen lock
systemctl --user enable --now facegate-watch # start the auto-unlock daemon
```

---

## How Screen Unlock Works

Facegate uses two complementary mechanisms:

### 1. PAM module — for sudo and login managers

```
sudo / SDDM / LightDM / GDM
  └─→ PAM
       └─→ pam_facegate.so
            └─→ facegate auth --user <name>  (subprocess)
                 └─→ ONNX Runtime (on-device)
```

The PAM module is called when the user initiates authentication — for example, when running `sudo` or pressing Login at the SDDM screen. If the face matches, PAM succeeds immediately; otherwise it falls through to the next module (password, if fallback is enabled).

The PAM module spawns `facegate auth` as a separate subprocess so the module itself carries no ML dependencies and remains small and auditable.

### 2. Watch daemon — for screen lock (Windows Hello style)

```
logind
  └─→ Lock signal (D-Bus)
       └─→ facegate-watch (user daemon)
            └─→ opens camera via logind session ACLs
            └─→ face recognised → loginctl unlock-session
```

`facegate-watch` is a systemd user service that subscribes to `org.freedesktop.login1.Session.Lock` on the system D-Bus. The moment the screen locks, it opens the camera and starts recognising. If the face matches, it calls `org.freedesktop.login1.Session.Unlock()` directly — no keypress needed.

If recognition fails or times out, the daemon stops the camera and lets the user type their password normally. If the user types their password first (the `Unlock` signal fires), any ongoing scan is cancelled immediately.

**This is architecturally equivalent to Windows Hello:** a dedicated process reacts to a system event, the user never needs to interact with an unlock form, and the camera is released as soon as a decision is made.

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

When enrolling with `--for session` or `--for both`, the template directory is automatically `chown`ed to the enrolled user so that `facegate-watch` (which runs as the user, not as root) can read the templates.

```
$ sudo facegate add mart --for both
How many samples do you want to capture? [1-10, default 3]: 3

Enrolling sudo+session face for 'mart' (label: 'mart', 3 sample(s))
Opening camera and loading models...

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
| `doctor` | Check installation status |
| `cameras` | List `/dev/video*` and flag IR vs RGB |
| `camera-test [--device DEV]` | Test camera and face detection |
| `add USERNAME [--label LABEL] [--for sudo\|session\|both]` | Enroll face templates |
| `list USERNAME` | List enrolled templates |
| `remove USERNAME ID` | Remove a template by ID |
| `test USERNAME [--for sudo\|session\|all]` | Live recognition test |
| `session-auth` | Toggle face auth in login/session PAM services |
| `completions SHELL` | Print shell completion script |

All commands except `completions`, `cameras`, and the internal `watch`/`auth`
helpers require root. `cameras` and `watch` run as the normal user.

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

[logging]
level = "warn"
log_failed_attempts = true
```

---

## Architecture

```
facegate/
├── crates/
│   ├── facegate_core/   # camera, detection, embedding, matching, storage, config
│   ├── facegate_cli/    # CLI + TUI + watch daemon (facegate binary)
│   └── pam_facegate/    # PAM module (pam_facegate.so)
├── packaging/
│   └── nfpm/            # .deb / .rpm / .pkg.tar.zst manifests + postinstall
├── scripts/
│   └── package-nfpm.sh  # one-shot multi-distro package builder
├── systemd/
│   └── facegate-watch.service
├── docs/
│   └── facegate.1
└── install-dev.sh
```

**`facegate_core`** handles the full ML pipeline: V4L2 capture, SCRFD face detection, ArcFace embedding extraction, cosine similarity matching, and secure template storage.

**`facegate_cli`** provides the `facegate` binary with a Clap CLI and a Ratatui TUI. It also implements the `watch` subcommand — the D-Bus daemon used by the systemd service.

**`pam_facegate`** is a minimal cdylib that implements `pam_sm_authenticate`. It spawns `facegate auth` as a subprocess so the PAM module itself carries no ML or async dependencies, making it small and auditable.

---

## Security

### What Facegate is

Facegate is a **convenience authentication mechanism**. It is designed to reduce friction for everyday operations — unlocking your screen, running `sudo` — without replacing the security of a strong password. Password authentication remains available at all times (unless you explicitly disable fallback).

### What it is not

- It is not a replacement for hardware-backed authentication (TPM, FIDO2, smartcard).
- It does not implement liveness detection. A high-quality photograph of the enrolled user could theoretically fool the camera depending on the model and threshold. For high-security scenarios, keep face auth as a first factor and require password confirmation for sensitive operations.

### No network, no cloud

All processing happens on your machine. Face embeddings are computed locally by ONNX Runtime. No image, embedding, or identity data is ever sent over the network. There is no telemetry.

### Template storage

Face templates are stored as ArcFace embedding vectors — 512 floating-point numbers that represent a mathematical fingerprint of a face. They are not photographs and cannot be used to reconstruct a face image.

- Templates are stored under `/var/lib/facegate/users/<username>/embeddings.json`
- Permissions: `0600` (readable only by the file owner)
- Enrollment (root) writes the file and immediately `chown`s it to the enrolled user for session-auth flows
- All writes are atomic (write to `.tmp`, `fsync`, `rename`) — no partial state on power loss
- Symlink traversal is blocked on all file operations

**Note on embedding exfiltration.** Because the file is owned by the enrolled
user, the user (or any process running as that user) can read their own ArcFace
vector. Vectors are not photos, but a sufficiently capable adversary could use
them to drive an image-generation model. This is acceptable in our threat model
— same-UID code is already trusted — but worth being aware of.

### PAM module

`pam_facegate.so` is deliberately minimal. It does not link ONNX Runtime, does not load models, and does not open the camera. It spawns `/usr/bin/facegate auth --user <name>` as a subprocess and interprets its exit code. A 45-second hard timeout ensures PAM is never blocked indefinitely regardless of what happens to the subprocess.

### Watch daemon attack surface

`facegate-watch` has no listening socket of any kind — it cannot be reached over the network or via a local Unix socket. Its only input is D-Bus signals from the system bus.

**D-Bus signal authenticity.** The `Lock` and `Unlock` signals the daemon listens to are emitted by `org.freedesktop.login1`, the service name owned exclusively by systemd-logind, which runs as root. An unprivileged process cannot claim that service name on the system bus. A process running as a different user cannot inject fake `Lock` signals — the D-Bus daemon verifies sender credentials using kernel socket credentials (`SO_PEERCRED`). Only systemd-logind can trigger a recognition scan.

**Unlocking.** When a face is recognised, the daemon calls the `Unlock()` method on the `org.freedesktop.login1.Session` object for the current session. This is authorised by polkit because the caller owns the session (same UID, same session ID). A process in a different session or with a different UID cannot call `Unlock()` on someone else's session.

**Camera access without the `video` group.** The daemon runs as the logged-in user within their active logind session. systemd-logind + udev automatically grant the active session's owner access to `/dev/video*` through filesystem ACLs set at session activation time. The `video` group is not required and is not added. This is the proper Linux session permission model — the same one used by PipeWire, PulseAudio, and other session-aware daemons.

**Same-UID attacker.** If an attacker already has code execution as the same user, the session is already fully compromised and the daemon adds nothing to the attack surface. They already have the same filesystem access, the same camera access, and the same D-Bus access.

### Comparison with Windows Hello

Windows Hello uses `WinBioSvc`, a system service running as `SYSTEM`, which holds exclusive access to the IR camera. User processes never touch the camera hardware. Facegate's watch daemon achieves an architecturally equivalent separation: the daemon is a distinct process reacting to a system event (D-Bus `Lock`), camera access is mediated by logind session ACLs rather than explicit group membership, and the user's password credentials are never involved in the face-recognition path.

The key difference is that `facegate-watch` runs in the user's session (not as SYSTEM), which is why it can access session-scoped resources like the camera without elevated privileges and without bypassing the Wayland portal model.

### Summary

| Property | Sudo auth | Watch daemon |
|---|---|---|
| Runs as | root (via PAM) | user (session daemon) |
| Camera access | direct (root) | logind session ACLs |
| `video` group needed | no | no |
| Network exposure | none | none |
| D-Bus exposure | none | subscriber only (no listener) |
| Forging a trigger | N/A — user initiates | requires impersonating systemd-logind (impossible for unprivileged code) |
| Template readable by | root + enrolled user | root + enrolled user |

---

## License

Facegate is licensed under the GNU General Public License v3.0 or later.

You are free to use, study, modify, and redistribute the software. Modified
redistributions must preserve the same open-source freedoms under the GPL.
See [LICENSE](LICENSE) for details.
