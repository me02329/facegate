# Camera support

Facegate uses **V4L2** for capture and supports any device that
appears under `/dev/video*` with a usable format.

## Format support

| V4L2 fourcc | Kind | Used as |
|---|---|---|
| `YUYV` | RGB | Primary RGB device |
| `MJPG` | RGB (JPEG) | Primary RGB device (decoded on the broker) |
| `GREY` / `Y8  ` / `Y800` | IR (8-bit grayscale) | Secondary IR device (cross-check liveness signal) |

`facegate cameras` enumerates devices and labels each one based on
the formats it reports. Devices that only report grayscale formats
are flagged as IR; devices that report YUYV or MJPG are flagged as
RGB.

## Choosing the right primary camera

Facegate's recognition pipeline (ArcFace) is RGB-trained. **Always
pick an RGB device as the primary `[camera].device`**. If your laptop
has both an RGB webcam and an IR camera, the IR one belongs in
`[camera.ir]`, not in `[camera]`.

```sh
sudo facegate setup       # auto-picks RGB as primary; offers cross-check inline
# or
sudo facegate cameras     # list devices then edit /etc/facegate/config.toml
```

## RGB+IR cross-check

If both an RGB and an IR sensor are present, the cross-check turns
the IR feed into a liveness signal:

1. Both cameras capture a frame at roughly the same time.
2. The broker rejects probes whose RGB/IR capture timestamps disagree
   by more than `max_time_skew_ms` (default 200 ms).
3. SCRFD must detect exactly one face in **each** stream.
4. Landmarks from the IR detection are mapped to RGB pixel space via
   a homography you calibrate once with `facegate calibrate-cameras`.
5. If the mapped IR landmarks are more than `max_position_offset_px`
   away from the RGB landmarks, the probe is rejected.

The IR stream is **not** used to compute an embedding. See the [FAQ
entry][faq-ir] and the [IPC protocol page](../architecture/ipc.md)
for the rationale.

## Calibration

```sh
sudo facegate calibrate-cameras --rgb-device /dev/video0 \
                                --ir-device /dev/video2 \
                                --samples 30 --write --enable
```

This captures `--samples` RGB+IR landmark pairs in parallel scoped
threads, estimates the IR→RGB homography, reports reprojection error,
writes the calibration to `/etc/facegate/config.toml`, and (with
`--enable`) sets `[camera.cross_check].enabled = true`.

If the reprojection error is high (typically > 10 px at 480p), try
again with the user's face at varying positions/distances during
sampling. A high error usually means the two cameras have very
different fields of view (a wide-angle IR cam and a narrow RGB cam,
for instance) and the homography is fundamentally a poor fit.

[faq-ir]: ../faq.md#is-this-windows-hello-for-linux
