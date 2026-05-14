# Troubleshooting

## "Authentication failed" but the camera lit up

Check the local diagnostic log first:

```sh
facegate logs --lines 30
```

It records coarse outcomes — `camera error`, `timeout`, `cross-check
reject`, `mismatch`, `not enrolled` — without exposing frames or
embeddings. If the line says `mismatch`, your enrolled templates and
the live capture have drifted apart. Common causes:

- Different lighting than enrolment (see the
  [FAQ entry on low light][faq-lowlight]).
- Glasses on/off, beard, hair in front of the face.
- Camera moved relative to the previous setup.

Fix: enrol additional templates under the current conditions
(`sudo facegate add <user> --label "with-glasses"`) or recalibrate
the threshold (`sudo facegate calibrate <user> --for session`).

## Broker socket "unavailable"

```sh
facegate broker status
```

If the systemd unit is inactive or the socket file is missing:

```sh
sudo facegate broker restart
journalctl -u facegate-brokerd.service -n 50 --no-pager
```

If the socket is present but `facegate broker health` fails, the
broker is running but rejecting your IPC version (`VersionMismatch`).
The CLI and broker must be installed from the same package; reinstall
both from the same release.

## Cross-check rejects every attempt

Inspect `facegate logs` for the reject reason:

- `cross_check_time_skew` — the two cameras' timestamps disagree by
  more than `[camera.cross_check].max_time_skew_ms`. Increase the
  skew tolerance, or check whether one camera is dropping frames on
  the first capture (warmup_frames too low).
- `cross_check_position_offset` — IR landmarks don't map onto the
  RGB face after the homography. Recalibrate with `facegate
  calibrate-cameras --samples 30 --write`.
- `cross_check_face_count` — SCRFD detected zero or two faces in one
  of the streams. Adjust framing.

## "Permission denied" on `embeddings.json` or `audit.log`

The broker owns these files as `facegate:facegate` with mode `0600`.
If a manual edit or a botched migration left them with the wrong
ownership, repair:

```sh
sudo facegate broker repair-permissions
```

This is idempotent and safe to run repeatedly.

## PAM rejecting every login (lockout)

See [Recovery and emergency
disable](./security/recovery.md). The short path from another TTY:

```sh
sudo facegate emergency-disable --dry-run     # see what would change
sudo facegate emergency-disable               # apply
```

If you have no other TTY available, the recovery page covers
single-user mode, chroot from a live USB, and manual PAM file edits.

## ONNX Runtime errors at boot

```sh
journalctl -u facegate-brokerd.service | grep -i 'onnxruntime\|libonnx'
```

Common causes:

- `libonnxruntime.so` missing — re-run the postinstall script or
  reinstall the package; the interactive download default is **no**
  so a Ctrl-D during install skips the runtime.
- SHA mismatch — the postinstall verifies the runtime checksum and
  aborts on mismatch; a corrupted download leaves the broker without
  inference and the service fails fast.

## Doctor

When in doubt:

```sh
facegate doctor
```

`doctor` checks library presence, model files, broker socket
reachability, PAM stack state, and prints actionable hints. It does
**not** require root.

[faq-lowlight]: ./faq.md#why-does-facegate-fail-to-recognise-me-in-the-dark
