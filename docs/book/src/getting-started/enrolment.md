# Enrolment

A *template* is an ArcFace embedding computed from a capture of your
face. The broker stores one or more templates per (username, scope)
pair under `/var/lib/facegate/users/<username>/embeddings.json`, mode
`0600`, owned by `facegate:facegate`.

## Adding templates

```sh
sudo facegate add <USERNAME> --for sudo,session --label "default"
```

`--for` selects the auth scopes the templates apply to:

- `sudo` — used for `sudo`, `su`, and any PAM service you've wired up
  with stricter recognition policy.
- `session` — used for login managers (SDDM, GDM, LightDM, greetd) and
  the screen-unlock daemon.
- `both` — shorthand for `sudo,session`.

`--label` is free-form text shown in `facegate list` and the TUI
templates browser. Enrol multiple labelled templates per scope to
cover varied lighting, glasses on/off, etc. The broker matches against
every template for the (user, scope) pair — any one of them can
succeed.

## Listing and removing

```sh
facegate list <USERNAME>            # list templates via the broker
sudo facegate remove <USERNAME> <ID>  # remove one template by ID
sudo facegate forget <USERNAME>     # remove every template for a user
```

`forget` asks for confirmation by default; pass `--yes` to skip.

## Sample count and threshold

By default `facegate add` captures one frame per invocation. The TUI
enrolment flow exposes a sample count selector (1–10) and captures
that many frames in sequence, each saved as a separate template. This
is the easiest way to build a robust template set without writing a
loop.

The recognition threshold lives in `[recognition.session]` and
`[recognition.sudo]`. Defaults aim for low false-accept on sudo
(stricter) and low false-reject on session unlock (looser). Calibrate
both with:

```sh
sudo facegate calibrate <USERNAME> --for sudo --samples 20 --write
sudo facegate calibrate <USERNAME> --for session --samples 20 --write
```

`calibrate` captures the requested number of positive samples, reports
score statistics, and (with `--write`) updates the per-scope threshold
in `/etc/facegate/config.toml`.
