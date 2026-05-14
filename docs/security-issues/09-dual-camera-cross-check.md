# [Security] Add RGB+IR dual-stream cross-check as presentation attack defense

## Status

Implemented for the broker/client auth path as protocol v5. The IR stream is
used as a **liveness signal**, not as a second identity signal: the broker
requires exactly one face in IR at the position predicted by the homography,
but does not compare the RGB and IR embeddings against each other (the
embedder is trained on colour faces and produces meaningless similarities
across modalities). Calibration tooling is available via `facegate
calibrate-cameras`, which estimates the IR→RGB homography from live SCRFD
landmark pairs and can write it to the config. RGB and IR frames are captured
in parallel scoped threads and timestamped inside the camera layer, so the
broker's `max_time_skew_ms` window measures real capture skew.

## Problem

On laptops with both an RGB sensor and an IR mono sensor (e.g. typical Windows Hello hardware: ThinkPad, Dell Latitude, HP Elitebook with the Chicony 04f2:b829 module and its variants), Facegate currently picks one camera device via `[camera].device` and uses it in isolation. Two classes of presentation attacks remain open even after liveness PAD lands:

- IR-only attacks: a print or screen that reflects IR similarly to skin (e.g. high-quality matte print, IR-friendly silicone mask) can match in IR alone.
- RGB-only attacks: a colour photo or screen replay can match in RGB alone if the liveness model is fooled.

A genuine live face is the only thing that shows up as a real face in both streams at the same time and position. Windows Hello exploits this; Facegate has the hardware on most target machines but did not use it.

## Security goal

Reject probes where the face detected in the RGB stream and the face detected in the IR stream are not consistent in space and time. Most printed-photo and screen-replay attacks fail the IR step entirely (a screen emits IR poorly; matte paper reflects very differently from skin under the IR illuminator). This closes single-modality presentation attacks without requiring a depth sensor or a cross-modal embedder.

## Approach

Optional dual-stream capture and cross-check mode driven by config:

```toml
[camera]
device = "/dev/video0"          # RGB

[camera.ir]
device = "/dev/video2"          # IR mono (enables cross-check when present)
# width / height / fps / timeout_ms / warmup_frames / min_face_size
# default to IR-friendly values when omitted

[camera.cross_check]
enabled = true
max_position_offset_px = 40
max_time_skew_ms = 50
allow_identity_homography = false   # require real calibration
```

Pipeline:

1. Open both V4L2 devices and grab a frame from each **in parallel** scoped threads. Each frame is stamped with its capture instant inside `V4lCamera::capture_frame` so the broker measures real RGB↔IR skew.
2. Run SCRFD on both frames (using IR-specific `min_face_size` for the IR stream so a lower-resolution IR sensor still detects a face reliably). Reject if either stream has zero or more than one face.
3. Apply the configured homography mapping IR landmarks into RGB pixel coordinates. Reject if the mapped IR centroid is more than `max_position_offset_px` from the RGB centroid.
4. Match the **RGB** embedding against the enrolled template using the existing match logic. **No ArcFace run on IR** — its job is liveness, not identity.

All inference and consistency checks happen inside `facegate-brokerd`. The client submits both frames with `Request::MatchFramePair`.

### Why we don't compare RGB↔IR embeddings

ArcFace (`arcface_w600k_r50`) is trained on colour faces. Feeding it an IR
frame (grey-replicated into RGB channels) produces an embedding from a
low-density region of the embedding space whose cosine similarity to the
corresponding RGB embedding is typically 0.10–0.35 for a genuine pair —
nowhere near a useful identity threshold, and the value drifts further in
low light when RGB degrades but IR stays clean. A previous iteration of this
feature rejected anything below 0.55, which rejected every genuine user. A
cross-modal embedder (e.g. LightCNN trained on CASIA NIR-VIS 2.0) is the only
way to get meaningful RGB↔IR identity similarity; that is a separate project.

## Calibration

The homography mapping IR↔RGB depends on the physical offset and lens distortion of each laptop's specific module. Two options:

- Per-model calibration table shipped with Facegate, keyed by USB vendor:product ID. Pragmatic for the handful of popular Chicony/Realtek modules.
- One-time calibration step: `facegate calibrate-cameras` captures RGB+IR landmark pairs, computes the homography, and writes `[camera.cross_check].homography` to the main config.

Calibration is the long pole — without it the cross-check produces too many false rejects.

Current operator path:

1. Use `facegate cameras` to identify the RGB and IR nodes.
2. Set `camera.device` to RGB and add a `[camera.ir]` section with `device` pointing at the IR node.
3. Run `sudo facegate calibrate-cameras --ir-device /dev/videoN --write`.
4. Inspect the RMS/max reprojection errors; recapture if they are high.
5. Re-run with `--enable` only after `sudo facegate test <USER>` works reliably.

Validation refuses `cross_check.enabled = true` while the homography is still
the identity matrix unless the operator explicitly sets
`allow_identity_homography = true`. This forces the calibration step on
real hardware (where IR and RGB sensors are physically offset by ~3 cm).

## Open questions

- Should cross-check be enforced for sudo scope only, or for session scope as well? (Trades convenience for sudo strength.)
- How does this interact with liveness PAD from #06 — additive (both must pass) or fallback (cross-check alone is sufficient liveness signal)?
- Latency budget: parallel dual capture + dual SCRFD + single ArcFace adds one SCRFD pass over the single-camera path. Acceptable for PAM auth (~1s) and the screen-unlock daemon.

## Acceptance criteria

- [x] Config accepts `[camera.ir]` and `[camera.cross_check]` and validates them.
- [x] Broker can ingest a synchronized RGB+IR frame pair and produce one match decision.
- [x] Tests cover: missing second stream (fail closed), unsynchronized frames, position mismatch, and the position-consistency happy path.
- [x] Calibration path is documented and implemented as `facegate calibrate-cameras`.
- [x] Auth path remains usable on single-camera systems (cross-check disabled).
- [x] Identity-matrix homography is refused at config validation when cross-check is enabled (unless explicitly allowed).

Remaining follow-up:

- Test and publish presets for common Chicony/Realtek laptop modules.
- Improve the calibration UX with a visual target / quality score once a GUI or preview flow exists.

## Out of scope

- Depth-based anti-spoofing (Z16 / Intel RealSense, structured light). Almost no Linux laptop exposes depth via V4L2; tracked separately if ever pursued.
- Synchronization at hardware level (genlock, trigger lines). Not exposed on consumer modules.
