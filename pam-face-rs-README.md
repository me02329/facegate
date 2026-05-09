# pam-face-rs

`pam-face-rs` is a native Rust facial authentication stack for Linux, designed as a modern, safer and maintainable alternative to legacy PAM facial authentication tools based on Python, `pam-python`, Python 2, and fragile native bindings.

The project provides:

- a native PAM module written in Rust;
- a standalone CLI for enrollment, testing, diagnostics and administration;
- camera support for RGB and IR sensors through Linux video devices;
- face detection and embedding extraction through ONNX Runtime;
- configurable matching policies;
- secure local storage of biometric templates;
- clean integration with `sudo`, and later with login managers such as SDDM.

The initial target platform is **Arch Linux + KDE Plasma + Wayland**, but the design should remain portable across modern Linux distributions.

---

## Project status

This project is currently in design / MVP phase.

The first milestone is not to replace all biometric authentication mechanisms, but to solve the most painful part of existing Linux face authentication stacks:

```text
PAM → pam_python.so → Python code → dlib/face_recognition → fragile build/runtime dependencies
```

with:

```text
PAM → pam_face_rs.so → Rust helper → ONNX Runtime → face detection + embedding comparison
```

---

## Goals

### Primary goals

- Provide a native Rust PAM integration without `pam-python`.
- Avoid Python 2 entirely.
- Avoid mandatory CUDA/cuDNN dependencies.
- Support IR cameras exposed as `/dev/videoX`.
- Work reliably with `sudo` as the first supported PAM target.
- Provide a clean CLI for enrollment, testing and diagnostics.
- Store biometric templates securely.
- Fail closed when the system is misconfigured.
- Preserve normal password fallback when configured through PAM as `sufficient`.

### Non-goals for the MVP

- Full Windows Hello equivalence.
- Hardware-backed biometric trust chain.
- Kernel-level camera security.
- Anti-spoofing guarantees equivalent to commercial biometric stacks.
- Immediate SDDM/login-screen integration.
- Cloud sync of biometric templates.

---

## Security model

`pam-face-rs` is intended as a **convenience authentication mechanism**, not as a replacement for strong authentication.

Recommended use:

```text
auth sufficient pam_face_rs.so
auth include system-auth
```

This means:

- if facial authentication succeeds, PAM authentication succeeds;
- if facial authentication fails, PAM continues to the normal password flow;
- if the module errors, times out, or cannot access the camera, password fallback remains available.

This is safer than making facial authentication mandatory.

### Threat model

The project should assume:

- the local user may use the feature for convenience;
- the machine may be a laptop;
- the camera may be an IR camera, but not necessarily a depth sensor;
- a determined attacker with physical access may attempt spoofing;
- the biometric template database must not be world-readable;
- the module must not crash PAM or lock the user out.

### Security limitations

`pam-face-rs` does **not** claim to provide:

- Windows Hello security guarantees;
- hardware-backed liveness detection;
- protection against all replay/spoofing attacks;
- protection if an attacker already has root access;
- protection if the PAM stack is misconfigured.

For high-security operations, prefer:

- strong password;
- FIDO2 / security key;
- smartcard;
- disk encryption;
- hardware-backed authentication.

---

## Architecture

Recommended architecture:

```text
pam-face-rs/
├── crates/
│   ├── pam_face_rs/        # PAM module: pam_face_rs.so
│   ├── face_rs_core/       # camera, detection, embedding, matching
│   └── face_rs_cli/        # CLI: face-rs
├── models/
│   ├── scrfd_500m.onnx
│   └── arcface.onnx
├── packaging/
│   └── arch/
│       └── PKGBUILD
├── docs/
│   ├── SECURITY.md
│   ├── PAM.md
│   └── CAMERA.md
├── README.md
└── Cargo.toml
```

Runtime layout:

```text
/usr/lib/security/pam_face_rs.so
/usr/bin/face-rs
/etc/face-rs/config.toml
/usr/share/face-rs/models/scrfd_500m.onnx
/usr/share/face-rs/models/arcface.onnx
/var/lib/face-rs/users/<username>/embeddings.json
```

---

## Components

### `pam_face_rs`

Native PAM module.

Responsibilities:

- read PAM username;
- call the privileged authentication helper;
- enforce timeout;
- return appropriate PAM result;
- avoid doing heavy ML inference inside the PAM module itself.

The PAM module should stay small and auditable.

Expected PAM line:

```text
auth sufficient pam_face_rs.so
```

### `face_rs_core`

Core library.

Responsibilities:

- open camera device;
- capture frames;
- preprocess frames;
- detect face;
- align/crop face;
- extract embedding;
- compare embeddings;
- apply matching policy.

### `face_rs_cli`

Administration CLI.

Expected commands:

```bash
face-rs doctor
face-rs camera-test
face-rs add
face-rs test
face-rs list
face-rs remove
face-rs auth
```

---

## Authentication flow

```text
sudo
  ↓
PAM
  ↓
pam_face_rs.so
  ↓
/usr/bin/face-rs auth --user <username>
  ↓
open configured camera
  ↓
capture frames until timeout
  ↓
detect face
  ↓
extract embedding
  ↓
compare against enrolled embeddings
  ↓
exit code 0 / 1 / 2 / 3
  ↓
PAM success or fallback
```

Exit codes:

```text
0 = recognized
1 = not recognized
2 = timeout
3 = camera error
4 = configuration error
5 = internal error
```

---

## Face recognition pipeline

The recognition pipeline should follow a standard embedding-based approach:

```text
camera frame
  → face detection
  → face alignment/crop
  → embedding extraction
  → cosine similarity
  → threshold decision
```

Recommended model strategy:

- SCRFD or similar lightweight ONNX model for face detection;
- ArcFace-compatible ONNX model for embeddings;
- CPU inference by default;
- optional acceleration later.

### Matching

Cosine similarity should be used for comparing embeddings.

Pseudo-code:

```rust
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    dot / (norm_a * norm_b)
}

fn is_match(current: &[f32], enrolled: &[Vec<f32>], threshold: f32) -> bool {
    enrolled
        .iter()
        .any(|known| cosine_similarity(current, known) >= threshold)
}
```

Threshold must be configurable.

Example:

```toml
[recognition]
threshold = 0.55
```

The correct value depends on model, camera, lighting and enrollment quality.

---

## Configuration

Default config path:

```text
/etc/face-rs/config.toml
```

Example:

```toml
[camera]
device = "/dev/video2"
width = 640
height = 360
fps = 30
timeout_ms = 5000
warmup_frames = 3

[recognition]
threshold = 0.55
required_matches = 1
max_attempts = 3
min_face_size = 80

[models]
detector = "/usr/share/face-rs/models/scrfd_500m.onnx"
embedder = "/usr/share/face-rs/models/arcface.onnx"

[storage]
base_dir = "/var/lib/face-rs/users"

[logging]
level = "info"
log_failed_attempts = true

[security]
allow_password_fallback = true
deny_on_camera_error = false
```

---

## File permissions

Biometric templates must not be readable by regular users.

Recommended permissions:

```text
/etc/face-rs/config.toml                  root:root 0644
/usr/bin/face-rs                          root:root 0755
/usr/lib/security/pam_face_rs.so          root:root 0755
/usr/share/face-rs/models/*.onnx          root:root 0644
/var/lib/face-rs                          root:root 0755
/var/lib/face-rs/users                    root:root 0755
/var/lib/face-rs/users/<username>         root:root 0700
/var/lib/face-rs/users/<username>/*.json  root:root 0600
```

The enrollment command should require root privileges because it writes into `/var/lib/face-rs`.

---

## CLI design

### Diagnostics

```bash
face-rs doctor
```

Expected checks:

```text
✓ config file exists
✓ configured camera exists
✓ camera can be opened
✓ frame capture works
✓ detector model exists
✓ embedder model exists
✓ ONNX Runtime loads
✓ PAM module is installed
✓ user has enrolled templates
✓ template permissions are safe
```

### Camera test

```bash
face-rs camera-test --device /dev/video2
```

Should show or capture diagnostic frames and report:

```text
device: /dev/video2
resolution: 640x360
format: YUYV/MJPEG/GRAY
frame capture: OK
face detected: YES/NO
```

### Enrollment

```bash
sudo face-rs add mart --label mart-normal
sudo face-rs add mart --label mart-glasses
sudo face-rs add mart --label mart-low-light
```

Enrollment should store one or more embeddings per label.

### Listing models

```bash
sudo face-rs list mart
```

Example output:

```text
Known face models for mart:

ID  Created              Label
0   2026-05-09 14:14:03  mart-normal
1   2026-05-09 14:25:51  mart-glasses
2   2026-05-09 14:26:07  mart-far
```

### Testing

```bash
sudo face-rs test mart
```

Should perform live capture and report best match:

```text
Detected face
Best match: mart-normal
Similarity: 0.63
Threshold: 0.55
Result: ACCEPT
```

### PAM authentication helper

```bash
/usr/bin/face-rs auth --user mart
```

This command should be non-interactive and return only exit codes suitable for PAM.

---

## PAM integration

### sudo

Recommended first integration target:

```bash
sudo cp /etc/pam.d/sudo /etc/pam.d/sudo.backup
sudo nano /etc/pam.d/sudo
```

Add at the top:

```text
auth sufficient pam_face_rs.so
```

Example:

```text
auth sufficient pam_face_rs.so
#%PAM-1.0
auth        include     system-auth
account     include     system-auth
session     include     system-auth
```

Test:

```bash
sudo -k
sudo whoami
```

Expected result:

```text
root
```

If facial auth fails, password fallback should remain available.

### SDDM / login screen

Login manager integration is not recommended until `sudo` integration is stable.

Do not enable SDDM integration by default.

---

## Rust workspace

Recommended workspace:

```toml
[workspace]
resolver = "2"
members = [
  "crates/face_rs_core",
  "crates/face_rs_cli",
  "crates/pam_face_rs",
]
```

Recommended crates:

```toml
[dependencies]
anyhow = "1"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = "0.3"
clap = { version = "4", features = ["derive"] }
nix = "0.29"
libc = "0.2"
```

For ONNX:

```toml
ort = "2"
ndarray = "0.16"
```

For camera support, evaluate:

```toml
v4l = "0.14"
```

or OpenCV bindings if needed:

```toml
opencv = "0.94"
```

The project should prefer a lightweight V4L2 path first, then add OpenCV only if required for image preprocessing.

---

## Development setup

### Arch Linux dependencies

Install Rust and system dependencies:

```bash
sudo pacman -S --needed \
  rustup \
  base-devel \
  clang \
  pkgconf \
  cmake \
  git \
  v4l-utils \
  opencv \
  onnxruntime \
  linux-headers
```

Initialize Rust:

```bash
rustup default stable
rustup component add rustfmt clippy
```

Optional useful tooling:

```bash
cargo install cargo-watch
cargo install cargo-audit
cargo install cargo-deny
cargo install cargo-nextest
```

### Create the project

```bash
mkdir pam-face-rs
cd pam-face-rs

cargo new crates/face_rs_core --lib
cargo new crates/face_rs_cli --bin
cargo new crates/pam_face_rs --lib
```

Create root `Cargo.toml`:

```bash
cat > Cargo.toml <<'EOF2'
[workspace]
resolver = "2"
members = [
  "crates/face_rs_core",
  "crates/face_rs_cli",
  "crates/pam_face_rs",
]
EOF2
```

### PAM module crate type

In `crates/pam_face_rs/Cargo.toml`, configure the library as a shared object:

```toml
[lib]
crate-type = ["cdylib"]
```

The final build artifact should be renamed/installed as:

```text
pam_face_rs.so
```

---

## Build

Debug build:

```bash
cargo build
```

Release build:

```bash
cargo build --release
```

Format:

```bash
cargo fmt
```

Lint:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Tests:

```bash
cargo test --workspace
```

Security audit:

```bash
cargo audit
```

---

## Local installation for development

Build release binaries:

```bash
cargo build --release
```

Install CLI:

```bash
sudo install -Dm755 target/release/face-rs /usr/bin/face-rs
```

Install PAM module:

```bash
sudo install -Dm755 target/release/libpam_face_rs.so /usr/lib/security/pam_face_rs.so
```

Create config directory:

```bash
sudo mkdir -p /etc/face-rs
sudo mkdir -p /usr/share/face-rs/models
sudo mkdir -p /var/lib/face-rs/users
```

Install example config:

```bash
sudo install -Dm644 config.example.toml /etc/face-rs/config.toml
```

Fix permissions:

```bash
sudo chown -R root:root /etc/face-rs /usr/share/face-rs /var/lib/face-rs
sudo chmod 755 /var/lib/face-rs
sudo chmod 755 /var/lib/face-rs/users
```

---

## Arch packaging

Initial AUR package name:

```text
pam-face-rs-git
```

Expected package contents:

```text
/usr/bin/face-rs
/usr/lib/security/pam_face_rs.so
/etc/face-rs/config.toml
/usr/share/face-rs/models/
```

The package should **not** automatically modify PAM files.

Instead, post-install instructions should say:

```text
To enable for sudo, add this line at the top of /etc/pam.d/sudo:

auth sufficient pam_face_rs.so
```

This avoids locking users out.

---

## Safety requirements

Before merging any PAM-related change:

- password fallback must work;
- authentication timeout must be enforced;
- camera errors must not crash PAM;
- config parse errors must not crash PAM;
- missing model files must fail cleanly;
- missing user enrollment must fail cleanly;
- logs must not expose biometric embeddings;
- biometric templates must not be world-readable;
- tests must cover match/no-match/error paths.

---

## Roadmap

### Phase 1: MVP CLI

- `face-rs camera-test`
- `face-rs add`
- `face-rs list`
- `face-rs test`
- local embedding storage

### Phase 2: PAM module

- `pam_face_rs.so`
- `face-rs auth --user`
- sudo integration
- timeout handling
- logging

### Phase 3: Packaging

- Arch PKGBUILD
- install paths
- example config
- post-install instructions

### Phase 4: Hardening

- stricter permissions
- better logs
- configurable policies
- rate limiting
- better error reporting

### Phase 5: Desktop integration

- SDDM optional support
- KDE lockscreen notes
- systemd service/helper if needed

---

## Rust commands quick start

Install dependencies:

```bash
sudo pacman -S --needed \
  rustup base-devel clang pkgconf cmake git \
  v4l-utils opencv onnxruntime linux-headers

rustup default stable
rustup component add rustfmt clippy

cargo install cargo-watch cargo-audit cargo-deny cargo-nextest
```

Create the workspace:

```bash
mkdir pam-face-rs
cd pam-face-rs

cargo new crates/face_rs_core --lib
cargo new crates/face_rs_cli --bin
cargo new crates/pam_face_rs --lib

cat > Cargo.toml <<'EOF2'
[workspace]
resolver = "2"
members = [
  "crates/face_rs_core",
  "crates/face_rs_cli",
  "crates/pam_face_rs",
]
EOF2
```

Configure the PAM module crate:

```bash
cat >> crates/pam_face_rs/Cargo.toml <<'EOF2'

[lib]
crate-type = ["cdylib"]
EOF2
```

Run first checks:

```bash
cargo build
cargo fmt
cargo clippy --workspace --all-targets
cargo test --workspace
```

Install locally later:

```bash
cargo build --release

sudo install -Dm755 target/release/face-rs /usr/bin/face-rs
sudo install -Dm755 target/release/libpam_face_rs.so /usr/lib/security/pam_face_rs.so

sudo mkdir -p /etc/face-rs
sudo mkdir -p /usr/share/face-rs/models
sudo mkdir -p /var/lib/face-rs/users
```

---

## Suggested initial Codex prompt

```text
Build the MVP of pam-face-rs following README.md. Start with the Rust workspace structure, config loading, CLI skeleton using clap, and a face_rs_core abstraction with stubbed camera/detection/matching interfaces. Do not implement PAM first. Prioritize testable modules, clean errors, and secure file layout.
```

---

## Disclaimer

`pam-face-rs` is a convenience authentication mechanism. It should not be treated as equivalent to Windows Hello, FIDO2, smartcards, or hardware-backed authentication systems.

Use password fallback.  
Do not deploy as the only authentication factor on sensitive systems.  
Do not enable login manager integration before testing `sudo` integration thoroughly.
