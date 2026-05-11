# [Security] Add RGB+IR dual-stream cross-check as presentation attack defense

## Priority

Low. Target milestone 0.3.0, after broker-side `MatchFrame` (closed), liveness PAD (#06), and TPM sealing (#07) are in place.

## Problem

On laptops with both an RGB sensor and an IR mono sensor (e.g. typical Windows Hello hardware: ThinkPad, Dell Latitude, HP Elitebook with the Chicony 04f2:b829 module and its variants), Facegate currently picks one camera device via `[camera].device` and uses it in isolation. Two classes of presentation attacks remain open even after liveness PAD lands:

- IR-only attacks: a print or screen that reflects IR similarly to skin (e.g. high-quality matte print, IR-friendly silicone mask) can match in IR alone.
- RGB-only attacks: a colour photo or screen replay can match in RGB alone if the liveness model is fooled.

A genuine live face is the only thing that should match consistently in both streams at the same time and position. Windows Hello exploits this with a proprietary RGB+IR consistency check; Facegate has the hardware on most target machines but does not use it.

## Security goal

Reject probes where the face detected in the RGB stream and the face detected in the IR stream are not consistent in space and identity at the same point in time. This closes single-modality presentation attacks without requiring a depth sensor.

## Proposed approach

Add an optional dual-stream capture and cross-check mode driven by config:

```toml
[camera]
device = "/dev/video0"      # RGB
ir_device = "/dev/video2"   # IR mono (enables cross-check when present)

[camera.cross_check]
enabled = true
max_position_offset_px = 40
max_time_skew_ms = 50
min_identity_similarity = 0.55
```

Pipeline changes:

1. Open both V4L2 devices simultaneously and grab a frame from each within `max_time_skew_ms` using V4L2 timestamps (no hardware sync needed; soft-sync via timestamp is acceptable for ~30 fps).
2. Run SCRFD on both frames. Reject if either has no face or more than one face.
3. Apply a per-model homography mapping IR landmarks into RGB pixel coordinates. Reject if the mapped IR centroid is more than `max_position_offset_px` from the RGB centroid.
4. Run ArcFace on both crops. Reject if cosine similarity between the two embeddings is below `min_identity_similarity` (loose, since RGB vs IR of the same face are not identical).
5. Match the RGB embedding against the enrolled template using the existing match logic.

All inference and consistency checks happen inside `facegate-brokerd`. The client submits both frames (extending `Request::MatchFrame` to `Request::MatchFramePair` or adding an optional second `FrameProbe` field).

## Calibration

The homography mapping IR↔RGB depends on the physical offset and lens distortion of each laptop's specific module. Two options:

- Per-model calibration table shipped with Facegate, keyed by USB vendor:product ID. Pragmatic for the handful of popular Chicony/Realtek modules.
- One-time calibration step: `facegate calibrate-cameras` shows a target on screen, captures synchronized pairs, computes the homography, stores it under `/etc/facegate/cross-check.toml`.

Calibration is the long pole — without it the cross-check produces too many false rejects.

## Open questions

- Should cross-check be enforced for sudo scope only, or for session scope as well? (Trades convenience for sudo strength.)
- How does this interact with liveness PAD from #06 — additive (both must pass) or fallback (cross-check alone is sufficient liveness signal)?
- Latency budget: dual capture + dual SCRFD + dual ArcFace roughly doubles inference cost. Acceptable for PAM auth (~1s) but tight for the screen-unlock daemon.

## Acceptance criteria

- Config accepts `ir_device` and `[camera.cross_check]` and validates them.
- Broker can ingest a synchronized RGB+IR frame pair and produce one match decision.
- Tests cover: missing second stream (fail closed), unsynchronized frames, position mismatch, identity mismatch, and the happy path with paired live frames.
- Calibration path is documented; tested on at least one widely available module (Chicony 04f2:b829 or equivalent).
- Auth path remains usable on single-camera systems (cross-check disabled or `ir_device` absent).

## Out of scope

- Depth-based anti-spoofing (Z16 / Intel RealSense, structured light). Almost no Linux laptop exposes depth via V4L2; tracked separately if ever pursued.
- Synchronization at hardware level (genlock, trigger lines). Not exposed on consumer modules.
