# Roadmap and known limitations

What ships today is summarised in the [threat
model](./threat-model.md). This page is the *not-yet-shipped* side:
gaps that the project knows about, why they aren't fixed yet, and
which milestone tracks each one.

## Liveness / presentation attack detection (PAD)

**Status:** partial — RGB+IR cross-check ships in v0.3.0 as a
*liveness signal*, not a PAD model. Tracked as
[#25][issue-25] (v0.4.0).

**Problem:** A high-quality printed photograph, a phone display, or
a 3D mask can still produce a face that SCRFD detects and ArcFace
embeds close enough to a real template to pass on a single-camera
setup. The RGB+IR cross-check raises the bar — an attacker now has to
spoof two streams that align spatially — but it is not the same as a
purpose-built anti-spoofing model.

**Plan:** integrate a lightweight PAD model (MiniFASNet,
Silent-Face-Anti-Spoofing, or equivalent ONNX) that runs alongside
ArcFace inside the broker. Reject low-quality, low-resolution,
screen-like, or flat presentation artefacts before they reach the
matcher. Config gates the threshold per scope.

## TPM2 sealing of templates at rest

**Status:** not started. Tracked as [#26][issue-26] (v0.4.0).

**Problem:** Templates live under `facegate:facegate` mode `0600`,
which blocks the enrolled user's own processes from reading them.
Offline disk access (cold-boot attacks, evil-maid scenarios with
disk removal, full-disk-encryption bypass via leaked key) is still
in scope: an attacker with the bytes can extract embeddings.

**Plan:** generate a random DEK, seal/wrap it with TPM2 bound to a
conservative PCR policy, encrypt `embeddings.json`, and only let the
broker unseal. A robust re-seal flow for firmware / bootloader /
kernel / initramfs updates is the hard part — PCR policies are
brittle and an unrecoverable lockout on a routine update is worse
than the protection is worth.

## Recognition robustness in varied lighting

**Status:** in progress. Tracked as [#51][issue-51] (v0.5.0).

**Problem:** The current pipeline feeds the aligned face crop to
ArcFace with no illumination normalisation, and the enrolment UX
prompts for multiple samples without guiding the user to vary
conditions between captures. The result is that templates cluster
around the lighting of one environment, and a user moving to a
different room (or to a different time of day) bounces off
authentication until they re-enrol. See the [recognition pipeline
page][rp] for the chain of cause and effect.

**Plan:** add CLAHE-style illumination normalisation in
`facegate_core::embedding` before ArcFace inference, and rewrite the
per-sample prompts in `facegate add` (and the TUI enrolment screen) to
actively instruct the user to change lighting, distance, and head pose
between samples. Default sample count moves from 3 to 5. The same
preprocessing runs at enrolment and at match time so they cannot
drift.

This is the cheap robustness win — no model change required — and it
is a prerequisite for any honest benchmark of later work.

## Model licensing — InsightFace bundle replacement

**Status:** not started. Tracked as [#52][issue-52] (v0.4.0).

**Problem:** The packaging postinstall script and `install-dev.sh`
both download the **`buffalo_l.zip` bundle** from
`github.com/deepinsight/insightface/releases/` and extract the
detector (SCRFD) and embedder (ArcFace `w600k_r50`) we ship as
defaults. The InsightFace project documents that its pre-trained
models are released for **non-commercial research use only**, even
though the surrounding code is MIT. Facegate code is GPL-3.0-or-later
and binary packages are distributed publicly (GitHub Releases, AUR via
`facegate-bin`, COPR). We do not redistribute the `.onnx` files inside
the packages, but our install scripts and shipped configuration
actively prescribe and fetch them on every install — which is not a
defensible position for a public OSS project.

**Plan:** switch the defaults to permissively-licensed alternatives:

- **AuraFace-v1** (Apache-2.0) for the embedder. Explicitly built as a
  commercial-clean ArcFace alternative; same 112×112 RGB input,
  512-d output, so it drops into the existing pipeline.
- **OpenCV YuNet** (MIT) for face detection.

The swap is mandatory before any benchmarking against future work, so
any numbers captured are against the embedder we will actually ship.
Existing users will need to re-enrol because embeddings from different
models are not comparable; this is documented in the issue.

## IR-native and multi-modal recognition

**Status:** open research direction. Tracked as [#16][issue-16]
(v0.5.0), blocked by [#51][issue-51] and [#52][issue-52].

**Problem:** ArcFace is RGB-trained. In low light, the RGB stream is
noisy and the IR stream is unusable for matching because the same
ArcFace weights produce meaningless similarities on IR crops — a fact
that was empirically confirmed in commit `1582696` when the previous
IR cross-check was changed from identity matching to liveness-only.
See the [recognition pipeline page][rp] for why and how Windows Hello
sidesteps this with an IR-trained model paired with an active IR
illuminator.

**Audit finding (May 2026):** the project does not have a candidate
IR-native open-source model. The audit looked at HuggingFace,
InsightFace community forks, the OpenCV Zoo, and academic releases
(PR-HFR, LightCNN variants); nothing combines (a) NIR-trained weights,
(b) a permissive licence Facegate can ship, and (c) production-grade
accuracy. The canonical academic NIR dataset (CASIA NIR-VIS 2.0) is
research-only with explicit no-redistribution and no-commercial-use
terms. Every peer Linux project we looked at (Howdy, Visage,
LinuxCamPAM, Biopass) ultimately runs an RGB-trained embedder against
IR frames — nobody on the Linux side has shipped IR-native identity
matching with open models.

**Plan:** [#16][issue-16] has therefore been reframed from *"prepare a
model swap"* into *"empirically determine how far the IR pipeline can
be pushed through the (post-#52) RGB embedder, with the (post-#51)
illumination normalisation applied to IR crops, behind a clean backend
abstraction"*. Concretely the issue will:

- Build a controlled benchmark on the test hardware (RGB-only,
  IR-only, and current RGB+IR cross-check configurations) and
  document the operating points (FAR / FRR by lighting condition).
- Land a trait-based backend abstraction
  (`trait FaceEmbedder` / `trait FaceDetector`) so future model swaps
  do not touch call sites across the codebase.
- If IR-through-RGB-embedder reaches usable territory, ship a new
  `ir-primary` profile with sane per-scope recognition defaults.
- If it does not, document the failure modes and trigger the
  long-term fallback below.

### Long-term fallback: custom IR model via synthetic data

If [#16][issue-16]'s empirical work shows that the existing RGB
embedder cannot deliver acceptable identity matching on IR frames,
the realistic next step is **not** to find a different pretrained
NIR model — the May 2026 audit confirmed none exists under a licence
we can ship. The realistic step is to **train one ourselves on
synthetic NIR data**, generated from RGB faces we already have
permission to use.

**Why synthetic and not real NIR data:** every NIR face dataset
worth training on (CASIA NIR-VIS 2.0, Oulu-CASIA NIR-VIS,
PolyU-NIRFD, BUAA-VisNir, HFB) is academic and released under
research-only terms with explicit no-redistribution and
no-commercial-use clauses. We cannot ship a model whose weights are
derived from any of them in a public GPL package. Collecting our own
NIR dataset is technically possible but requires hundreds-to-thousands
of consenting subjects under GDPR Article 9 (special-category
biometric data), controlled lighting setups, and months of work —
not a realistic path for this project.

**The synthetic NIR pipeline (clean licence chain end-to-end):**

```text
   commercially-clean        public method            own training
   RGB face dataset    +     (PBFR / similar)    →    on synthetic NIR
   (e.g. AuraFace's          for RGB → NIR             via AuraFace
   training data)            transformation            fine-tuning
        │                          │                        │
        ▼                          ▼                        ▼
     Apache-2.0                code public,             our weights,
     compatible              method published          our licence
                              (NeurIPS 2022)            (Apache-2.0)
```

**Method candidates:**

- **PBFR** (Physically-Based Face Rendering for NIR-VIS Face
  Recognition, NeurIPS 2022, `github.com/yoqim/PR-HFR`) — reconstructs
  3D face shape + reflectance from a 2D RGB face, transforms the VIS
  reflectance into NIR reflectance via a physical model, then renders
  photorealistic synthetic NIR. **Code is public**; only the
  pre-trained weights live in the InsightFace repo and inherit its
  non-commercial restriction. We re-train using the published
  methodology on data we control → resulting weights are clean.
- **CycleGAN-style NIR↔VIS translation** — simpler implementation,
  more variable output quality, used as a fallback if PBFR turns out
  to be too heavy to reproduce.

**Honest caveat:** synthetic NIR is not real NIR. The model may
under-perform on actual IR sensor output (precise wavelength
differences, sensor noise, illuminator calibration). We will only
know after benchmarking on the test hardware. If synthetic-only
training falls short, the next step would be to negotiate
research-use access to CASIA NIR-VIS 2.0 specifically for *evaluation*
(not training) — that is a much narrower ask than a redistributable
training licence.

**Estimated cost:** several GPU-days of compute (a single A100 cloud
instance or a local consumer GPU is sufficient), a few weeks of ML
engineering time for the pipeline + fine-tuning + evaluation. The
critical-path risk is dataset prep and evaluation, not raw compute.

This is captured in #16's acceptance criteria as the path forward if
the empirical IR evaluation fails — a separate issue with its own
milestone would be opened at that point.

## Multi-camera fallback

**Status:** not started. Tracked as [#10][issue-10] (v0.4.0).

**Problem:** Today `[camera].device` is a single device. If that
device is busy, missing, or fails to capture, auth fails — no second
chance, no automatic switch.

**Plan:** extend the camera config to a prioritised list, prefer IR
when available, fall back to RGB if the IR device times out. The
chosen device is surfaced in `status` and the audit log. Important
caveat: this is independent of *IR-native recognition* (above) — even
with fallback, low light remains hard until a model swap happens.

## Per-context authentication policy

**Status:** partial — `[recognition.sudo]` and `[recognition.session]`
scopes ship in v0.3.0. Full per-context policy tracked as
[#8][issue-8] (v0.4.0).

**Problem:** Two scopes is the minimum useful split, but it doesn't
let an operator say *"face auth only for screen unlock, never for
sudo"* or *"different thresholds for KDE vs SDDM"* without manually
editing PAM files.

**Plan:** extend the policy config so each PAM service can declare
its own threshold, required matches, fallback behaviour, and cooldown
settings. Existing two-scope configs continue to work as defaults.

[issue-8]: https://github.com/me02329/facegate/issues/8
[issue-10]: https://github.com/me02329/facegate/issues/10
[issue-16]: https://github.com/me02329/facegate/issues/16
[issue-25]: https://github.com/me02329/facegate/issues/25
[issue-26]: https://github.com/me02329/facegate/issues/26
[issue-51]: https://github.com/me02329/facegate/issues/51
[issue-52]: https://github.com/me02329/facegate/issues/52
[faq-lowlight]: ../faq.md#why-does-facegate-fail-to-recognise-me-in-the-dark
[rp]: ../architecture/recognition-pipeline.md
