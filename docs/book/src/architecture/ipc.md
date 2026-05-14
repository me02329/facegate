# IPC protocol

Facegate uses a versioned JSON-over-Unix-socket protocol between
clients (CLI, PAM helper, watch daemon) and the broker. The protocol
crate is `facegate_ipc`. The current version is **v5**.

The full reference lives in [`docs/ipc-protocol.md`][ipc-spec] in the
repo — request and response shapes, error codes, and version-bump
policy. Below is a short orientation.

## Envelope

```json
{
  "version": 5,
  "request": {
    "type": "match_frame",
    "username": "alice",
    "auth_scope": "sudo",
    "frame": {
      "width": 640,
      "height": 480,
      "format": "yuyv",
      "captured_at_ms": 1715692800123,
      "bytes": "<base64>"
    }
  }
}
```

The base64 encoding for `bytes` avoids the 4× bloat of integer-array
serialisation. The maximum request size is 12 MB, which covers a
1080p RGB frame.

## Version handshake

Clients are rejected with `VersionMismatch` if `version` does not
match the broker's `PROTOCOL_VERSION`. Pin the broker and the CLI
together at install time; this is why the same package ships both.

## Auth scopes (`auth_scope`)

- `sudo` — used for `sudo`, `su`, and any PAM service wired up with
  stricter recognition policy.
- `session` — used for login managers and the watch daemon.

The scope drives which `[recognition.<scope>]` policy applies on the
broker side (threshold, required_matches, max_attempts).

## RGB+IR cross-check

When `[camera.cross_check].enabled = true`, clients submit a
`MatchFramePair` instead of `MatchFrame`. The pair carries two frames
captured close together in time. The broker rejects probes whose
capture timestamps disagree by more than `max_time_skew_ms` (default
200 ms), or whose RGB / IR streams do not contain exactly one face,
or whose mapped landmark positions exceed `max_position_offset_px`.

The IR stream is used as a **liveness signal** (face presence +
spatial alignment), not for cross-modal identity matching. See the
[FAQ entry](../faq.md#why-does-facegate-fail-to-recognise-me-in-the-dark)
for the reason.

[ipc-spec]: https://github.com/me02329/facegate/blob/master/docs/ipc-protocol.md
