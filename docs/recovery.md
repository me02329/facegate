# Facegate Recovery Guide

This guide is for recovering when face authentication breaks sudo, login, or
screen unlock. Password authentication should remain available, but PAM mistakes
can still make recovery stressful.

## Before changing PAM

Always keep a second root shell open before enabling or editing PAM:

```bash
sudo -v
sudo -s
```

Leave that shell open while you test `sudo`, login, and screen locking in a
separate terminal/session.

## Recovery from a working shell

If you still have a shell with sudo/root access, run the emergency rollback:

```bash
sudo facegate emergency-disable --dry-run
sudo facegate emergency-disable
```

This command:

- restores the newest Facegate PAM backup that does not contain
  `pam_facegate.so`;
- removes any remaining `pam_facegate.so` lines from `/etc/pam.d/*`;
- disables/stops `facegate-brokerd.service`;
- disables/stops the current user's `facegate-watch` service when possible.

If you prefer to do the rollback manually:

```bash
sudo facegate session-auth
sudo systemctl disable --now facegate-brokerd.service
systemctl --user disable --now facegate-watch
```

Then inspect `/etc/pam.d/*.facegate.*.bak` and restore the most recent backup
for each affected PAM file that does not contain `pam_facegate.so`.

## Recovery from a TTY

If the graphical session no longer unlocks:

1. Press `Ctrl+Alt+F3` to open a text TTY.
2. Log in with your password.
3. Run:

   ```bash
   sudo facegate emergency-disable
   sudo reboot
   ```

If the normal boot target immediately starts a broken display manager, add this
temporarily from GRUB:

```text
systemd.unit=multi-user.target
```

Boot, log in on the text console, run the emergency command, then reboot.

## Recovery from chroot or live USB

Use this path when you cannot get any working shell on the installed system.

1. Boot a live USB.
2. Mount the installed root filesystem:

   ```bash
   sudo mount /dev/<root-partition> /mnt
   ```

3. If the system has a separate boot partition, mount it too:

   ```bash
   sudo mount /dev/<boot-partition> /mnt/boot
   ```

4. Restore PAM manually:

   ```bash
   ls /mnt/etc/pam.d/*.facegate.*.bak
   sudo cp /mnt/etc/pam.d/<service>.facegate.<timestamp>.bak /mnt/etc/pam.d/<service>
   ```

Choose backups that do not contain `pam_facegate.so`:

```bash
grep -L pam_facegate.so /mnt/etc/pam.d/*.facegate.*.bak
```

If no clean backup exists, remove the Facegate line from the affected PAM file:

```bash
sudoedit /mnt/etc/pam.d/<service>
```

Delete lines like:

```text
auth      sufficient    /usr/lib/security/pam_facegate.so
auth      sufficient    pam_facegate.so
```

## Distro notes

Arch:

- If you chroot, use `arch-chroot /mnt`.
- If boot files were touched during unrelated recovery, regenerate initramfs
  with `mkinitcpio -P` from the chroot.

Debian/Ubuntu:

- GDM commonly uses `/etc/pam.d/gdm-password` or `/etc/pam.d/gdm3`.
- SDDM and LightDM use `/etc/pam.d/sddm` and `/etc/pam.d/lightdm`.

Fedora:

- GDM commonly uses `/etc/pam.d/gdm-password`.
- PAM services can include authselect-managed files. Prefer
  `facegate emergency-disable` from a booted root shell when possible.

## Diagnose without making it worse

Read before editing:

```bash
sudo facegate status
sudo facegate doctor
journalctl -u facegate-brokerd.service
```

If available, `pamtester` lets you test a PAM service from an existing root
shell before logging out:

```bash
sudo pamtester sudo "$USER" authenticate
```

Keep the root shell open until password authentication works again.
