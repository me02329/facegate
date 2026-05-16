# Installation

Facegate ships as `.deb`, `.rpm`, and `.pkg.tar.zst` packages built by
`nFPM` in CI. All packages install a hardened `facegate-brokerd` system
service and the `facegate` CLI under `/usr/bin`.

## From GitHub Releases (recommended)

1. Pick the artifact for your distribution from the [releases
   page][releases].
2. Verify the checksum against `checksums.sha256` from the same release.
3. Install:

   ```sh
   # Debian / Ubuntu
   sudo dpkg -i facegate_*.deb

   # Fedora / RHEL
   sudo dnf install ./facegate-*.rpm

   # Arch Linux
   sudo pacman -U ./facegate-*.pkg.tar.zst
   ```

4. The postinstall script creates the `facegate:facegate` system user,
   `/var/lib/facegate` with the correct ownership, fetches the ONNX
   Runtime shared library and the YuNet + AuraFace ONNX model files
   from their authoritative HuggingFace mirrors, and enables
   `facegate-brokerd.service`.

   The interactive default for the ONNX runtime / model downloads is
   **no** — answer `y` when prompted (or pre-set
   `FACEGATE_ASSUME_YES=1`) if you accept the ~400 MB pull.

5. Verify the install:

   ```sh
   facegate status
   facegate doctor
   ```

[releases]: https://github.com/me02329/facegate/releases

## From source (development install)

Requires Rust 1.95.0 (pinned in `rust-toolchain.toml`), a C toolchain,
and the V4L2 / `libonnxruntime` development packages.

```sh
git clone https://github.com/me02329/facegate.git
cd facegate
sudo ./install-dev.sh
```

The dev installer mirrors what the packaging postinstall does: creates
the system user, installs the broker binary and unit file, migrates
template storage to broker ownership, and enables the service.

## Building packages yourself

```sh
cargo build --workspace --release
scripts/package-nfpm.sh
```

Output lands in `dist/`. nFPM v2.43.0+ is required; the project's
release CI fetches a pre-built binary from the nFPM GitHub release.
