# [Security] Correct docs: ArcFace templates are sensitive biometric secrets

## Priority

High, should be done with or before the broker work.

## Problem

The README currently says:

```text
They are not photographs and cannot be used to reconstruct a face image.
```

This is too strong. ArcFace embeddings are not photographs, but they are biometric templates. Treating them as non-reconstructable understates the risk and conflicts with the later note that a capable adversary could use them to drive image generation.

The Windows Hello comparison also overstates the current architecture. `facegate-watch` is a user service, not a privileged biometric broker. It is a distinct process, but it is not a distinct template trust boundary from the rest of the user session.

## Proposed wording

Replace the reconstruction claim with:

```text
Face templates are ArcFace embedding vectors: compact biometric templates derived from face images. They are not photographs, but they are sensitive biometric data. Published model-inversion and template-inversion techniques can sometimes produce face-like images or transferable biometric artifacts from embeddings, so Facegate treats templates as secrets.
```

Replace the current Windows Hello comparison with:

```text
Windows Hello uses a privileged biometric service and hardware-backed protections to keep biometric templates outside normal user processes. Current Facegate releases do not yet provide that level of isolation: session templates are readable by the enrolled user so the watch daemon can authenticate. The planned broker architecture moves templates to a dedicated `facegate` system user and exposes only match decisions over local IPC.
```

Also document that the broker architecture is still only a partial Windows Hello analogue if the MVP accepts client-computed probe embeddings:

```text
The broker prevents normal user processes from reading enrolled templates, but the first implementation may still accept probe embeddings computed by the client. That protects stored biometric templates from passive exfiltration, but it does not prove the probe came from a live camera frame. Full Windows Hello-style semantics require broker-side frame processing and liveness checks.
```

## Acceptance criteria

- README no longer says embeddings cannot be reconstructed.
- Man page no longer says embeddings cannot be reconstructed.
- README clearly distinguishes:
  - public model weights;
  - private enrolled biometric templates;
  - camera/live spoofing risk;
  - same-UID template exfiltration risk.
- Windows Hello language is accurate and does not claim equivalent isolation before the broker exists.
- Post-broker docs still describe the remaining limitation if matching is based on client-provided embeddings.
