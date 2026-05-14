# First-time setup

The fastest way to a working install is the guided flow:

```sh
sudo facegate setup
```

`facegate setup` walks through:

1. **Camera selection** — detects `/dev/video*` devices, flags IR vs
   RGB based on supported V4L2 formats, and writes the chosen device
   into `/etc/facegate/config.toml`.
2. **Enrolment** — captures multiple frames of your face (default 5)
   and submits them to the broker. The broker computes the ArcFace
   embeddings and stores them as templates owned by
   `facegate:facegate`.
3. **PAM wiring** — adds the Facegate PAM line to `sudo`, `login`, or
   any session PAM file you select. Backups of the original PAM files
   are saved alongside (suffix `.facegate.<timestamp>.bak`) so
   `facegate emergency-disable` can revert.
4. **Cross-check (optional)** — if both an RGB and an IR camera are
   detected, the flow offers to run `facegate calibrate-cameras` and
   enable `[camera.cross_check]`.

After setup, verify with:

```sh
facegate status
sudo facegate test <USERNAME> --for sudo
```

`status` reports broker reachability, camera presence, model presence,
template counts, and recent audit events. `test` runs a live recognition
attempt without invoking PAM.

## TUI alternative

Instead of `facegate setup`, you can run `sudo facegate` (no
subcommand) to open the interactive TUI. Every CLI surface that the
setup flow uses is also reachable from the menu, plus a live system
status panel that refreshes broker / watch / camera / audit indicators
in the right pane.

## Screen-unlock daemon (Windows Hello style)

To get auto-unlock when your session goes to the lock screen:

```sh
systemctl --user enable --now facegate-watch.service
```

The watch daemon listens to `org.freedesktop.login1` Lock / Unlock
signals on the system bus and submits a `MatchFrame` to the broker as
soon as the screen locks. See
[Screen unlock flow](../architecture/screen-unlock.md) for how it
interacts with KDE / GNOME / sway lock screens.
