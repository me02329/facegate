# FAQ

## Why do I need to re-enrol when I move to a different room?

Short answer: today's pipeline treats every meaningful change in ambient
lighting as if it were a different face. Enrolling in one environment
produces templates that drift away in the embedding space when the
runtime environment differs — so the match score falls under the
threshold, and you bounce off authentication until you re-enrol.

This is the same root cause as the next FAQ entry ("why does it fail in
the dark"), generalised: not just darkness, but *any* lighting change
big enough to shift the captured frame meaningfully. A warmer lamp at
night, sunlight pouring through one window in the afternoon, or a
brighter monitor as your only light source — each one moves your live
capture into a region of the embedding space the templates do not
cover.

Why this happens, what we are doing about it, and how Windows Hello
sidesteps it entirely is documented in detail on the [recognition
pipeline and lighting dependence][rp] architecture page. Short version
of the planned work:

- [#51][issue-51] adds illumination normalisation (CLAHE) before the
  embedder runs, plus enrolment UX that actively prompts you to vary
  conditions between samples. This is the cheap, no-model-change fix.
- [#52][issue-52] swaps the embedder for a permissively-licensed
  alternative (AuraFace, Apache-2.0). Required quality baseline for the
  next step.
- [#16][issue-16] then evaluates empirically whether routing the IR
  camera through the new pipeline gets close to Windows Hello's
  environmental independence.

**Workarounds today:**

- Enrol a few extra templates under your common conditions:
  `facegate add --label "morning"`, `--label "evening"`,
  `--label "lamp-only"`. The broker matches against every template
  you have for a user, so any one of them can succeed.
- Lower the `threshold` in `[recognition.session]` if you accept the
  trade-off (sudo defaults stay strict).

## Why does Facegate fail to recognise me in the dark?

This is the extreme case of the lighting-dependence problem above, and
an expected limitation of the current pipeline rather than a bug.

Facegate's face embedding model is **AuraFace v1
(`glintr100.onnx`)** since v0.4.0 (previously
`arcface_w600k_r50.onnx`), an ArcFace-family ResNet-100 trained on RGB
faces in reasonably-lit conditions. In low light:

- The RGB sensor produces noisy, low-contrast frames.
- YuNet (face detection) often fails to detect a face, or detects a
  poor-quality crop.
- Even when a face is detected, the embedding drifts far from the
  templates you enrolled in normal light, so the match score falls
  below the recognition threshold.

**Why not just use the IR camera?** The optional `[camera.ir]` section
exists, but the IR stream is used *only as a liveness signal* in the
RGB+IR cross-check (verifying that a face is present and spatially
aligned with the RGB capture). The embedder is trained on RGB pixels
and produces meaningless similarity scores against IR crops — so the
broker **does not** match against the IR embedding. Doing so would
reject every genuine user. The [recognition pipeline page][rp]
explains why in detail, including how Windows Hello solves it with an
IR-trained model and an active IR illuminator.

**What you can do today:**

- Increase camera exposure / gain via `v4l2-ctl --set-ctrl
  exposure_absolute=...` or your camera's UI.
- Increase `warmup_frames` in `[camera]` so auto-exposure has time to
  ramp up before capture.
- Enrol additional templates under different lighting conditions
  (`facegate add --label "low-light" ...`).
- Lower the `threshold` in `[recognition.session]` if you accept the
  trade-off (sudo defaults stay strict).

**What is planned:**

- [#51][issue-51] (v0.5.0) — illumination preprocessing + guided
  multi-sample enrolment. The cheap robustness wins.
- [#52][issue-52] (v0.4.0, shipped) — model swap to AuraFace + YuNet
  is now the default install. Unrelated to low light directly, but the
  baseline that everything downstream measures against.
- [#16][issue-16] (v0.5.0) — empirical evaluation of the IR camera
  path through the new pipeline + interchangeable backends. If the IR
  path does not work through an RGB-trained embedder, a long-term
  follow-up will track training a custom IR model.
- [#10][issue-10] (v0.4.0) — multi-camera fallback (try a second
  device if the first one fails). Does not solve low light by itself.

## Is this Windows Hello for Linux?

Not exactly. Windows Hello and Facegate share the same general idea —
on-device face authentication with a trust boundary that's harder to
attack than a user-space binary — but the implementations diverge:

| Aspect | Windows Hello | Facegate |
|---|---|---|
| Hardware requirement | Certified IR cameras + depth sensor | Any V4L2 camera (RGB and/or IR) |
| Face model | Microsoft-proprietary, IR-trained | Open-source AuraFace (ArcFace family, RGB-trained, Apache 2.0) |
| Identity sensor | IR + active IR illuminator → lighting-invariant | RGB → sensitive to ambient light (see [pipeline page][rp]) |
| Liveness | Hardware depth + IR | Optional RGB+IR spatial cross-check |
| Template storage | TPM-sealed | File system, broker-owned (TPM sealing tracked in [#26][issue-26]) |
| Trust boundary | Kernel-mode driver | User-mode broker daemon with systemd hardening |
| Distros | Windows | Linux (Ubuntu, Fedora, Arch via nFPM packages) |

The honest summary: **Facegate aims for a Windows-Hello-style UX with
the threat model adapted to what you can build on commodity Linux
hardware**. It is not a drop-in replacement and the security posture
differs in ways called out in the [threat model](./security/threat-model.md).
The [recognition pipeline page][rp] walks through the most user-visible
gap — Windows Hello's lighting-invariant behaviour comes from its active
IR illuminator paired with an IR-trained model, neither of which has an
off-the-shelf open-source equivalent today.

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

## Are the YuNet / AuraFace models the best available?

They're a reasonable open-source baseline. Until v0.3.x facegate
shipped the InsightFace `buffalo_l` bundle (SCRFD-500M + ArcFace
`w600k_r50`), whose pre-trained models are
[released for non-commercial research use only][insightface-licensing]
even though the surrounding code is MIT. [#52][issue-52] swapped both
out in v0.4.0:

- **AuraFace-v1** / `glintr100.onnx` (Apache-2.0, ResNet-100) for the
  embedder. It's the only ArcFace-family model we found that is
  trained on commercially-licensable data; the trade-off is ~261 MB
  on disk versus ~166 MB for the previous R50.
- **OpenCV YuNet** / `face_detection_yunet_2023mar.onnx` (MIT) for
  detection. 233 KB, anchor-free, designed for edge inference — much
  smaller than any SCRFD variant.

If you want to know which other research-grade models exist and why
none of them are silver bullets:

- **AdaFace** (2022) — adaptive-margin variant of ArcFace, ~1% better
  on hard benchmarks (low quality, profile angles). Licence inherits
  from InsightFace.
- **MagFace** — produces a magnitude correlated with face quality;
  useful for rejecting blurry frames before they reach the matcher.
- **InsightFace's bundled SCRFD variants** (`scrfd_500m`, `scrfd_10g`)
  — better accuracy/latency curve than YuNet but the same
  non-commercial licence story as the embedder they ship alongside.

[#16][issue-16] is the architectural prerequisite for swapping
backends so future model variants can be A/B tested without forking
the broker.

In practice the dominant factor on a working pipeline is **frame
quality** (lighting, exposure, blur, distance), not the model — which
is why [#51][issue-51] (illumination normalisation + guided enrolment)
is expected to beat any equivalent model swap on the typical user
problem. See the [recognition pipeline page][rp] for the full
breakdown.

[issue-10]: https://github.com/me02329/facegate/issues/10
[issue-16]: https://github.com/me02329/facegate/issues/16
[issue-25]: https://github.com/me02329/facegate/issues/25
[issue-26]: https://github.com/me02329/facegate/issues/26
[issue-51]: https://github.com/me02329/facegate/issues/51
[issue-52]: https://github.com/me02329/facegate/issues/52
[insightface-licensing]: https://www.insightface.ai/services/models-commercial-licensing
[rp]: ./architecture/recognition-pipeline.md
