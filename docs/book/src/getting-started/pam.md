# PAM configuration

Facegate authenticates through a standard Linux PAM module
(`pam_facegate.so`) installed at `/usr/lib/security/pam_facegate.so`.
The module is intentionally small — it spawns `facegate auth` as a
subprocess and forwards the result. ML, async runtime, and broker IPC
all live in the subprocess (and ultimately the broker), keeping the
PAM module itself short and auditable.

## Via the TUI (recommended)

```sh
sudo facegate
```

The Sudo Auth and Session Auth menu entries flip the corresponding PAM
service files on or off. Each edit creates a timestamped backup
(`<service>.facegate.<unix-time>.bak`) so you can recover if something
goes wrong.

## Manual setup

For `sudo`:

```pam
# /etc/pam.d/sudo (top of file)
auth   sufficient   /usr/lib/security/pam_facegate.so try_first_pass=1
auth   include      common-auth
```

For login (e.g. SDDM):

```pam
# /etc/pam.d/sddm (top of the auth stack)
auth   sufficient   /usr/lib/security/pam_facegate.so try_first_pass=1
auth   include      system-login
```

The `try_first_pass=1` option lets Facegate accept a password the user
already typed for a previous PAM module (avoids double prompts). On
`auth required` semantics: Facegate uses `sufficient` so a face match
short-circuits the rest of the stack, while a face *failure* falls
through to the password line below — matching the behaviour you'd
expect from a biometric factor that complements rather than replaces
the password.

## Supported session PAM services

Auto-detected and offered by the session-auth toggle:

- `sddm`, `sddm-greeter`
- `gdm-password`, `gdm-launch-environment`
- `lightdm`, `lightdm-greeter`
- `greetd`
- `login`, `system-login`
- `swaylock`, `i3lock`, `hyprlock` (where present)
- KDE screen-locker (`kde`)

Pass a custom service name with `sudo facegate session-auth add
<service>` if your distro uses something else.

## PAM helper timeout

The PAM helper subprocess has a **25-second** ceiling on the match
flow (camera open + frame capture + broker round-trip). This is short
enough that a failed face match falls through to the password prompt
without an awkward pause, and long enough to handle the warmup of
slower IR sensors. The previous 45 s timeout shipped in v0.1.0 felt
sluggish in practice.

## Recovery

If a PAM edit goes wrong and you can't log in, see
[Recovery and emergency disable](../security/recovery.md). The short
version: `sudo facegate emergency-disable --dry-run` from any other
TTY shows what would change, and dropping `--dry-run` restores the
backups and stops the broker.
