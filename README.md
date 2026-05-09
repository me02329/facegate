# Facegate

**Native facial authentication for Linux.**

Facegate is a PAM module and CLI tool that lets you unlock `sudo`, login sessions, and any PAM-aware service using your webcam. It runs entirely on-device — no cloud, no network, no external dependencies beyond a V4L2 camera and the ONNX Runtime.

---

## Features

- Face authentication via a standard Linux PAM module (`pam_facegate.so`)
- Interactive TUI to configure, enroll faces, run diagnostics, and toggle sudo auth
- Multi-sample enrollment with separate templates per capture for better accuracy
- ArcFace embeddings + SCRFD face detector (ONNX Runtime)
- Secure template storage: root-only, atomic writes, no symlink traversal
- Password fallback configurable per PAM service
- Shell completions for bash, zsh, fish

---

## Requirements

- Linux with V4L2 camera (`/dev/video*`)
- [ONNX Runtime](https://github.com/microsoft/onnxruntime) shared library (`libonnxruntime.so`)
- Root access for installation and enrollment

**Arch Linux:**
```bash
sudo pacman -S onnxruntime
```

---

## Installation

```bash
# 1. Build (as your normal user)
cargo build --release

# 2. Install (as root)
sudo bash install-dev.sh
```

The install script:
- Copies `facegate` to `/usr/bin/facegate`
- Installs `pam_facegate.so` to `/usr/lib/security/`
- Creates `/etc/facegate/`, `/usr/share/facegate/models/`, `/var/lib/facegate/users/`
- Installs the config, man page, and shell completions
- Downloads face recognition models (~400 MB) unless `--skip-models` is passed

---

## Quick Start

```bash
# Open the interactive menu (requires root)
sudo facegate

# Or use subcommands directly:
sudo facegate doctor                        # check installation
sudo facegate add $USER                     # enroll your face
sudo facegate test $USER                    # verify recognition works
```

---

## CLI Reference

```
facegate [--config PATH] [COMMAND]
```

| Command | Description |
|---|---|
| *(none)* | Open the interactive TUI menu |
| `configure` | Edit settings in a terminal UI |
| `doctor` | Check installation status |
| `camera-test [--device DEV]` | Test camera and face detection |
| `add USERNAME [--label LABEL]` | Enroll face templates for a user |
| `list USERNAME` | List enrolled templates |
| `remove USERNAME ID` | Remove a template by ID |
| `test USERNAME` | Live recognition test |
| `completions SHELL` | Print shell completion script |

All commands except `completions` require root.

---

## Enrollment

When enrolling, Facegate asks how many samples to capture (1–10, default 3). Each sample is saved as a separate template, which improves recognition across varying poses and lighting conditions.

```
$ sudo facegate add mart
How many samples do you want to capture? [1-10, default 3]: 3

Enrolling face for 'mart' (label: 'mart', 3 sample(s))
Opening camera and loading models...

Sample 1/3 — look at the camera, then press Enter...
Capturing (timeout: 5000ms)...
  ✓ template #0 saved (label: 'mart-1')

Sample 2/3 — look at the camera, then press Enter...
...
Done — 3 template(s) enrolled for 'mart'.
```

---

## PAM Setup

### Enable via the TUI (recommended)

Run `sudo facegate` and select **Sudo Auth** to toggle face authentication for `sudo` on/off. The menu shows the current state and updates immediately.

### Manual setup

Add the following line to `/etc/pam.d/sudo` (or any other PAM service) **before** the existing `auth` lines:

```
auth      sufficient    pam_facegate.so
```

> **Warning:** Always keep a root shell open while editing PAM configuration. A broken PAM config can lock you out of `sudo` and login.

### How it works

```
sudo  →  PAM  →  pam_facegate.so  →  /usr/bin/facegate auth --user <name>  →  ONNX Runtime
```

The PAM module spawns `facegate auth` as a subprocess to keep the module itself free of ML dependencies. If the face is recognized, PAM succeeds immediately. If not, it falls through to the next PAM module (usually password auth, if fallback is enabled).

---

## Configuration

The default config file is `/etc/facegate/config.toml`. Edit it with `sudo facegate configure` or directly.

```toml
[camera]
device = "/dev/video0"
width = 640
height = 480
fps = 30
timeout_ms = 5000
warmup_frames = 5

[recognition]
threshold = 0.35        # cosine similarity threshold (higher = stricter)
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
│   ├── facegate_cli/    # CLI + TUI (facegate binary)
│   └── pam_facegate/    # PAM module (pam_facegate.so)
├── docs/
│   └── facegate.1       # man page
└── install-dev.sh       # install script
```

**`facegate_core`** handles all the ML pipeline: V4L2 capture, SCRFD detection, ArcFace embedding extraction, cosine similarity matching, and secure template storage.

**`facegate_cli`** provides the `facegate` binary with a Clap CLI and a Ratatui TUI. The TUI covers enrollment, testing, configuration, diagnostics, and the sudo PAM toggle.

**`pam_facegate`** is a minimal cdylib that implements `pam_sm_authenticate`. It delegates authentication to `facegate auth --user <name>` to avoid loading ONNX Runtime inside the PAM process.

---

## Security Notes

- All commands require root — this prevents unprivileged enrollment attacks
- Template files are stored with `0600` permissions under `/var/lib/facegate/users/`
- Symlink traversal is blocked on all file operations
- Face recognition is a convenience mechanism, not a replacement for strong authentication
- Keep `allow_password_fallback = true` while testing and under degraded conditions (poor lighting, camera failure)
- Enroll multiple samples under realistic conditions for best accuracy

---

## License

MIT
