# [Security] Add liveness / presentation attack detection for camera-based auth

## Priority

High, after or in parallel with the broker work.

## Problem

Template isolation prevents passive exfiltration of enrolled embeddings, but it does not stop presentation attacks:

- printed photo;
- replayed video;
- phone/tablet display;
- generated face image;
- 3D mask;
- crafted embedding submitted to an embedding-based broker API.

Facegate currently relies on SCRFD + ArcFace matching only. There is no liveness or presentation attack detection (PAD).

## Security goal

Require evidence that the authentication probe comes from a live person in front of the camera, not from a static image, replay, generated frame, or offline-computed embedding.

## Proposed approach

Add liveness as a layered system:

- passive PAD model for every authentication attempt;
- challenge-response mode for higher-security flows;
- optional IR/depth-specific checks when hardware supports it;
- policy controls for sudo/session scopes.

Possible MVP:

- integrate a lightweight anti-spoofing model such as MiniFASNet / Silent-Face-Anti-Spoofing or an equivalent ONNX model;
- reject low-quality, low-resolution, screen-like, or flat presentation artifacts;
- expose config thresholds separately from ArcFace match thresholds;
- log only aggregate liveness decisions, never frames or embeddings.

Stronger follow-up:

- random blink/head-turn challenge;
- multiple-frame temporal consistency;
- IR camera preference and IR-specific anti-spoof checks;
- frame-based broker matching so liveness runs inside the broker, not inside a same-UID client.

## Relationship to broker work

If the first broker MVP accepts `probe_embedding`, broker-side liveness is impossible because the broker never sees the frame. Liveness can start in the client for UX and model validation, but it only becomes a trust-boundary defense once the broker supports `MatchFrame` and performs inference itself.

## Acceptance criteria

- Auth can reject a face match when liveness confidence is too low.
- Config supports enabling/disabling PAD and setting thresholds per scope.
- Tests cover liveness policy decisions separately from ArcFace match decisions.
- Docs clearly explain that liveness protects against photo/replay/mask classes, while the broker protects stored templates.
- Long-term design keeps a path to broker-side `MatchFrame`.

