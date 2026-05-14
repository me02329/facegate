# Screen unlock flow

The screen-unlock daemon is `facegate watch`, run as the user via a
`facegate-watch.service` systemd user unit.

## Activation

```sh
systemctl --user enable --now facegate-watch.service
```

Once enabled, the daemon listens on the **system D-Bus** for
`org.freedesktop.login1` Lock and Unlock signals on the current
session object path.

## What it does on Lock

1. Open the configured camera (`[camera].device`, plus `[camera.ir]`
   if cross-check is enabled).
2. Wait `warmup_frames` frames for auto-exposure to stabilise.
3. Capture a frame (or pair).
4. Build a `MatchFrame` / `MatchFramePair` envelope and submit to the
   broker over `/run/facegate/broker.sock`, with `auth_scope=session`.
5. On success: emit `loginctl unlock-session` via D-Bus (or the
   compositor's lock-screen API where applicable).
6. On failure: log the reason to the user's local diagnostic log
   (`~/.local/state/facegate/facegate.log`) and stop. The user falls
   back to typing their password.

## Why a separate process?

The watch daemon runs in the user's session manager, has access to
the V4L2 devices the user has access to, and uses the same broker IPC
path as the PAM helper. By keeping it small (just camera open +
capture + send + decode), the lock-screen window does not need any
ML or async runtime; the broker handles all of that.

This matches the Windows Hello UX — a system event triggers a
specific process to react, the user never interacts with an unlock
form, and the camera is released as soon as a decision is made.

Since v0.2.0 the watch daemon does **not** load SCRFD or ArcFace
either: like the PAM helper, it only captures a frame and submits it
to the broker. The detector and embedder live exclusively inside
`facegate-brokerd`.

## Supported lock screens

Detected and supported out of the box:

- KDE Plasma (KScreenLocker via systemd-logind signals)
- GNOME (gnome-shell via systemd-logind signals)
- Sway / Hyprland (`swaylock` / `hyprlock` integration through PAM)

If your compositor doesn't emit standard `logind` Lock signals, the
daemon won't trigger automatically. Tracked separately in #14
([Document supported display managers and lock screens][issue-14]).

[issue-14]: https://github.com/me02329/facegate/issues/14
