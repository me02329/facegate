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

## IR-native and multi-modal recognition

**Status:** not started. Architectural prerequisite is
[#16][issue-16] (v0.5.0) — interchangeable face model backends.

**Problem:** ArcFace is RGB-trained. In low light, the RGB stream is
useless and the IR stream is unusable for matching because the same
ArcFace weights produce meaningless similarities on IR crops. See the
[FAQ entry][faq-lowlight] for the full reasoning.

**Plan:** [#16][issue-16] adds the trait + dynamic loading so the
broker can run a different embedding backend per camera kind. Once
that lands, a follow-up issue will track integrating an IR-trained
ArcFace equivalent (or a multi-modal model that takes RGB+IR jointly).
Open-source IR face models with usable licences are scarce; the
project does not have a candidate yet.

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
[faq-lowlight]: ../faq.md#why-does-facegate-fail-to-recognise-me-in-the-dark
