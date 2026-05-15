# AUR packaging

This directory holds the upstream-maintained PKGBUILDs for Arch Linux.
They are not built by the GitHub release workflow — they get published
to the AUR by a maintainer after each release.

## Layout

- `facegate-bin/` — binary release. Pulls the `.deb` from the matching
  GitHub release, extracts the file tree into `$pkgdir`, and runs the
  `facegate.install` hook to create the system user, audit log, and
  systemd state. Fast install, no build dependencies.
- `facegate-git/` — source build from `master` HEAD. Useful for testing
  unreleased changes before a tag. Requires the Rust toolchain.

Both packages declare `conflicts` with the other so users cannot
accidentally install both.

## Publishing a new release to the AUR

For each bump (`facegate-bin` only — `facegate-git`'s `pkgver()` is
computed dynamically and only changes when HEAD moves):

1. Update `pkgver=` in `facegate-bin/PKGBUILD` to the new tag.
2. Refresh the source checksums (requires the artifacts to be live on
   the GitHub release):

   ```sh
   cd packaging/aur/facegate-bin
   updpkgsums
   ```

3. Regenerate `.SRCINFO` (AUR submission format):

   ```sh
   makepkg --printsrcinfo > .SRCINFO
   ```

4. Build locally and run a smoke test in a clean chroot to catch
   missing runtime deps (optional but recommended):

   ```sh
   extra-x86_64-build
   sudo pacman -U facegate-bin-*.pkg.tar.zst
   sudo facegate doctor
   ```

5. Push the `PKGBUILD`, `.SRCINFO`, and `facegate.install` to the
   `ssh://aur@aur.archlinux.org/facegate-bin.git` remote.

`facegate-git` follows the same flow but is normally only updated when
the dependencies or build/install steps change — not on every commit.

## Runtime dependencies

`facegate-bin` and `facegate-git` both `depends=('onnxruntime')`. Arch's
`extra/onnxruntime` package satisfies the broker's `load-dynamic` ort
backend — no manual download needed, unlike the .deb / .rpm flows.

Models (`scrfd_500m.onnx` + `arcface_w600k_r50.onnx`) are still fetched
on first run via `facegate doctor` / `facegate models install` because
their license forbids redistribution from third parties.
