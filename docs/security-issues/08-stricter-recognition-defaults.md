# [Security] Tighten recognition defaults for security-sensitive scopes

## Priority

Medium, independent of broker work.

## Problem

Facegate defaults currently favor convenience:

```toml
threshold = 0.55
required_matches = 1
max_attempts = 3
```

That may be acceptable for convenience unlock, but it is weak for higher-impact flows such as `sudo` or login.

## Security goal

Provide safer defaults and scope-specific policy knobs without making everyday unlock unusable.

## Proposed changes

- Support separate recognition policies for `sudo` and `session`.
- Use stricter defaults for `sudo` than for screen unlock.
- Consider requiring multiple successful captures for privileged flows.
- Make `required_matches >= 2` easy to enable and clearly documented.
- Add guidance for raising `threshold` after local calibration.

Potential default direction:

```toml
[recognition.session]
threshold = 0.55
required_matches = 1

[recognition.sudo]
threshold = 0.60
required_matches = 2
```

Exact values should be validated with real cameras before shipping.

## Acceptance criteria

- Config can express scope-specific threshold and required-match policy.
- Existing configs migrate cleanly.
- Docs explain FAR/FRR trade-offs in plain language.
- `facegate test` helps users calibrate thresholds without exposing stored embeddings.
- CI covers config parsing and fallback compatibility.

