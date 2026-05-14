# Contributing To Facegate

Facegate touches PAM, systemd, cameras, and biometric data. Test changes in a
VM or disposable machine when possible, and keep a root shell open whenever PAM
configuration is involved.

## Local Build

Install system dependencies for your distro. Typical packages include:

- Rust toolchain from `rust-toolchain.toml`
- `pkg-config`
- V4L2 development headers (`libv4l-dev`, `v4l-utils`, or distro equivalent)
- `clang` / `build-essential` or equivalent C toolchain

Build:

```bash
cargo build --release
```

The package postinstall downloads ONNX Runtime and model files. For development
installs, use `install-dev.sh` from a VM or machine where PAM rollback is safe.

## Tests And Checks

Run before opening a PR:

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

Targeted checks are useful while iterating:

```bash
cargo test -p facegate_core
cargo test -p facegate_cli
cargo test -p facegate_brokerd
```

## PAM-Safe Testing

Before enabling or editing PAM:

1. Open a second terminal.
2. Run `sudo -v && sudo -s`.
3. Keep that root shell open.
4. Test Facegate in another terminal.

Recovery commands:

```bash
sudo facegate emergency-disable --dry-run
sudo facegate emergency-disable
sudo facegate session-auth
```

For full recovery instructions, see `docs/recovery.md`.

## Packaging

Build release packages with:

```bash
scripts/package-nfpm.sh
```

Expected artifacts are `.deb`, `.rpm`, and `.pkg.tar.zst` packages under the
script's output directory. Packaging changes should be tested in at least one
fresh Arch-like and one Debian-like environment before release.

## Style

- Keep Rust formatted with `cargo fmt`.
- Prefer small, focused commits.
- Use existing module patterns before adding new abstractions.
- Do not make PAM changes without an obvious rollback path.

Commit prefixes commonly used in this repo:

- `feat:`
- `fix:`
- `docs:`
- `chore:`
- `test:`

Reference issues as `#N`. Use `Closes #N` / `Fixes #N` only when the commit
should close the issue after it reaches the default branch.
