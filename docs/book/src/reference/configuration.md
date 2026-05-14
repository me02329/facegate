# Configuration

The config file lives at `/etc/facegate/config.toml`. Edit it with
`sudo facegate configure` (TUI), `sudo facegate setup` (guided flow),
or any text editor.

```toml
[camera]
device = "/dev/video0"
width = 640
height = 480
fps = 30
timeout_ms = 5000
warmup_frames = 5
min_face_size = 80

# Optional secondary camera for the RGB+IR cross-check.
# When [camera.ir] is set and [camera.cross_check].enabled = true,
# the broker requires a synchronised MatchFramePair.
[camera.ir]
device = "/dev/video2"
# All keys are optional — IR-friendly defaults are applied when
# they're omitted (longer warmup_frames and timeout_ms, smaller
# min_face_size).

[camera.cross_check]
enabled = false
max_time_skew_ms = 200
max_position_offset_px = 60

[recognition.sudo]
threshold = 0.60
required_matches = 2
max_attempts = 5

[recognition.session]
threshold = 0.45
required_matches = 1
max_attempts = 10

[security]
cooldown_after_failures = 3
cooldown_seconds = 30

[models]
detector = "/usr/share/facegate/models/scrfd_500m.onnx"
embedder = "/usr/share/facegate/models/arcface_w600k_r50.onnx"

[storage]
base_dir = "/var/lib/facegate/users"
```

## Per-key reference

### `[camera]`

| Key | Default | Notes |
|---|---|---|
| `device` | `/dev/video0` | Primary camera path. Prefer RGB devices (YUYV / MJPG). |
| `width`, `height` | 640 × 480 | The broker validates declared geometry; max 4096². |
| `fps` | 30 | Stream FPS. Higher values shorten `warmup_frames` time. |
| `timeout_ms` | 5000 | Max time spent waiting for a usable frame. |
| `warmup_frames` | 5 | Frames discarded after `STREAMON` to let auto-exposure stabilise. |
| `min_face_size` | 80 | Minimum SCRFD face box dimension. Smaller boxes are ignored. |

### `[camera.ir]`

Same keys as `[camera]`, all optional. Sensible defaults: `timeout_ms`
and `warmup_frames` are larger than RGB (IR sensors are slower to
stabilise), `min_face_size` is 5/8 of the RGB value.

### `[camera.cross_check]`

| Key | Default | Notes |
|---|---|---|
| `enabled` | `false` | Requires `[camera.ir]` to be set when enabled. |
| `max_time_skew_ms` | 200 | RGB↔IR capture timestamp tolerance. |
| `max_position_offset_px` | 60 | After mapping IR landmarks via the calibrated homography to RGB pixel space, max allowed offset. |
| `allow_identity_homography` | `false` | Refuse to start with the identity homography — forces calibration. |

### `[recognition.sudo]` and `[recognition.session]`

| Key | sudo default | session default | Notes |
|---|---|---|---|
| `threshold` | 0.60 | 0.45 | Cosine similarity floor for ACCEPT. |
| `required_matches` | 2 | 1 | Number of independent captures that must each ACCEPT. |
| `max_attempts` | 5 | 10 | Per-user attempt budget before lockout. |

### `[security]`

| Key | Default | Notes |
|---|---|---|
| `cooldown_after_failures` | 3 | After this many consecutive failures, apply cooldown. |
| `cooldown_seconds` | 30 | Lockout duration after the threshold is hit. |
