# FAQ

## Why does Facegate fail to recognise me in the dark?

This is an expected limitation of the current pipeline, not a bug.

Facegate's face embedding model is **ArcFace
(`arcface_w600k_r50.onnx`)**, which is trained on RGB faces in
reasonably-lit conditions. In low light:

- The RGB sensor produces noisy, low-contrast frames.
- SCRFD (face detection) often fails to detect a face, or detects a
  poor-quality crop.
- Even when a face is detected, the embedding drifts far from the
  templates you enrolled in normal light, so the match score falls
  below the recognition threshold.

**Why not just use the IR camera?** The optional `[camera.ir]` section
exists, but the IR stream is used *only as a liveness signal* in the
RGB+IR cross-check (verifying that a face is present and spatially
aligned with the RGB capture). ArcFace is trained on RGB pixels and
produces meaningless similarity scores against IR crops — so the broker
**does not** match against the IR embedding. Doing so would reject
every genuine user.

**What you can do today:**

- Increase camera exposure / gain via `v4l2-ctl --set-ctrl
  exposure_absolute=...` or your camera's UI.
- Increase `warmup_frames` in `[camera]` so auto-exposure has time to
  ramp up before capture.
- Enrol additional templates under different lighting conditions
  (`facegate add --label "low-light" ...`). The broker matches against
  every template for a user, so any one of them can succeed.
- Lower the `threshold` in `[recognition.session]` if you accept the
  trade-off (sudo defaults stay strict).

**What is planned:**

- [#10][issue-10] (v0.4.0) — multi-camera fallback (try a second device
  if the first one fails). Does not solve low-light by itself.
- [#16][issue-16] (v0.5.0) — interchangeable face model backends. This
  is the architectural prerequisite for an IR-native or multi-modal
  recognition model.
- A future issue will track IR-native recognition specifically once
  [#16][issue-16] lands and a usable open-source IR ArcFace equivalent
  is identified.

## Is this Windows Hello for Linux?

Not exactly. Windows Hello and Facegate share the same general idea —
on-device face authentication with a trust boundary that's harder to
attack than a user-space binary — but the implementations diverge:

| Aspect | Windows Hello | Facegate |
|---|---|---|
| Hardware requirement | Certified IR cameras + depth sensor | Any V4L2 camera (RGB and/or IR) |
| Face model | Microsoft-proprietary, IR-trained | Open-source ArcFace (RGB-trained) |
| Liveness | Hardware depth + IR | Optional RGB+IR spatial cross-check |
| Template storage | TPM-sealed | File system, broker-owned (TPM sealing tracked in [#26][issue-26]) |
| Trust boundary | Kernel-mode driver | User-mode broker daemon with systemd hardening |
| Distros | Windows | Linux (Ubuntu, Fedora, Arch via nFPM packages) |

The honest summary: **Facegate aims for a Windows-Hello-style UX with
the threat model adapted to what you can build on commodity Linux
hardware**. It is not a drop-in replacement and the security posture
differs in ways called out in the [threat model](./security/threat-model.md).

## I locked myself out — how do I recover?

See [Recovery and emergency disable](./security/recovery.md). The short
version: from another TTY, run `sudo facegate emergency-disable
--dry-run` first to see what would change, then drop `--dry-run`.
For boot-time lockouts (PAM rejecting every login), the same page
covers single-user mode, chroot from a live USB, and manual PAM file
edits.

## Why is there a separate `facegate-brokerd` daemon?

The broker exists to move **biometric template ownership out of the
authenticated user's UID**. Before v0.2.0, templates lived under the
user's home and a same-UID attacker (any malware running as the user)
could exfiltrate them or replay a captured embedding into the matcher.
Since v0.2.0, templates live under `facegate:facegate` in
`/var/lib/facegate/users/`, mode `0600`, and the only path to a match
decision is `MatchFrame` (or `MatchFramePair` with cross-check) which
requires a real camera frame the broker re-runs inference on.

See the [broker architecture page](./architecture/broker.md) for the
full systemd hardening profile.

## When will TPM sealing / liveness PAD ship?

Both are tracked but not yet implemented:

- **TPM2 sealing of templates at rest** — [#26][issue-26]. Templates
  are currently protected by Unix file permissions but not sealed to
  platform state. A PCR-bound key would raise the bar against offline
  disk access. Tricky in practice (firmware/bootloader updates break
  PCR policies, so a robust re-seal/recovery flow has to ship before
  this becomes default).
- **Full liveness / presentation attack detection** — [#25][issue-25].
  v0.3.0 ships an RGB+IR cross-check that acts as a liveness *signal*
  (no two-camera attacker can spoof both streams trivially) but it is
  not a PAD model. A real PAD layer (MiniFASNet / Silent-Face-Anti-
  Spoofing or similar) would catch printed photos, replays, and 3D
  masks on single-camera setups.

## Are the SCRFD / ArcFace models the best available?

They're a reasonable open-source baseline, not the strict state of the
art. Notable alternatives:

- **AdaFace** (2022) — adaptive-margin variant of ArcFace, ~1% better
  on hard benchmarks (low quality, profile angles).
- **MagFace** — produces a magnitude correlated with face quality;
  useful for rejecting blurry frames before they reach the matcher.
- **ArcFace `w600k_r100`** — ResNet-100 backbone, ~1-2% better than
  `r50` but ~2x heavier.

For face detection, SCRFD-500M is a strong efficiency/accuracy tradeoff
and is unlikely to be a real bottleneck. [#16][issue-16] is the
architectural prerequisite for swapping backends so model variants can
be A/B tested without forking the broker.

In practice the bottleneck is **frame quality** (lighting, exposure,
blur, distance), not the model. Tuning the camera and enrolling under
varied conditions buys more than a model swap on typical hardware.

[issue-10]: https://github.com/me02329/facegate/issues/10
[issue-16]: https://github.com/me02329/facegate/issues/16
[issue-25]: https://github.com/me02329/facegate/issues/25
[issue-26]: https://github.com/me02329/facegate/issues/26
