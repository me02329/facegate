# COPR packaging (Fedora / RHEL / EPEL)

This directory holds the RPM spec used to build Facegate on the
[Fedora COPR](https://copr.fedorainfracloud.org/) build service. COPR
gives Fedora/RHEL users a real `dnf` repository:

```sh
sudo dnf copr enable me02329/facegate
sudo dnf install facegate
```

## What the spec builds

`facegate.spec` builds from the **source tarball** that the GitHub
release workflow attaches to every tag
(`facegate-<version>.tar.gz`). This is the same artifact downstream
distros would consume — building from it keeps COPR honest:

- The `facegate` and `facegate-brokerd` binaries (`cargo build --release`)
- `pam_facegate.so` under `/usr/lib64/security/`
- The shared `config.toml`, systemd units (system + user), man page
- Bash / Zsh / Fish completions
- `facegate` system user/group via `%pre` / scriptlets

Runtime dependencies (`onnxruntime`, `v4l-utils`) are pulled in by
`dnf`. The face-recognition models are *not* shipped in the RPM (their
license forbids redistribution); the user runs `sudo facegate doctor`
to fetch them on first launch.

## One-time COPR project setup

1. Sign in at <https://copr.fedorainfracloud.org/> with your Fedora
   account (FAS).
2. Create a new project named `facegate`. Suggested chroots:
   - `fedora-rawhide-x86_64`
   - `fedora-N-x86_64` (current stable)
   - `fedora-N-aarch64` (optional)
   - `epel-9-x86_64` (RHEL 9 / Alma / Rocky)
3. In **Packages → New package** select **Custom** source method (or
   **SCM**, which polls this repo).

### Custom-source build script (recommended)

The custom script runs in a minimal `mock` chroot, produces a SRPM, and
COPR builds it across every chroot. The script lives in COPR and just
needs to:

```sh
#!/bin/bash
set -euo pipefail
dnf install -y git rpm-build rpmdevtools curl

git clone --depth 1 --branch master https://github.com/me02329/facegate.git
cd facegate

VERSION="$(grep '^version' crates/facegate_cli/Cargo.toml | head -1 | cut -d'"' -f2)"
mkdir -p /tmp/rpmbuild/{SOURCES,SPECS,BUILD,RPMS,SRPMS}
curl -fsSL -o "/tmp/rpmbuild/SOURCES/facegate-${VERSION}.tar.gz" \
    "https://github.com/me02329/facegate/releases/download/v${VERSION}/facegate-${VERSION}.tar.gz"
cp packaging/copr/facegate.spec /tmp/rpmbuild/SPECS/

rpmbuild --define "_topdir /tmp/rpmbuild" -bs /tmp/rpmbuild/SPECS/facegate.spec
cp /tmp/rpmbuild/SRPMS/*.src.rpm "${RESULTDIR:-.}/"
```

Save this in COPR's "script" field. Build trigger: **on push to master
in this Git repo**.

### Tag-triggered builds via webhook (alternative)

COPR can be wired to GitHub via webhook so a tag push triggers a build
automatically. Settings → Integrations → GitHub on the COPR project,
then add the webhook URL in this repo's GitHub settings.

## Smoke-testing locally before pushing

```sh
sudo dnf install -y rpm-build rpmdevtools rust cargo clang pkgconf \
                    systemd-rpm-macros mock

rpmdev-setuptree
curl -fsSL -o "$HOME/rpmbuild/SOURCES/facegate-0.3.1.tar.gz" \
    https://github.com/me02329/facegate/releases/download/v0.3.1/facegate-0.3.1.tar.gz
cp packaging/copr/facegate.spec "$HOME/rpmbuild/SPECS/"

rpmbuild -ba "$HOME/rpmbuild/SPECS/facegate.spec"
sudo dnf install "$HOME/rpmbuild/RPMS/x86_64/facegate-*.rpm"
```

## Why a separate spec instead of reusing the nfpm-built RPM

nfpm produces a valid RPM but with synthetic metadata — no `BuildRoot`,
no scriptlet macros, no `Requires` resolved against the build chroot.
Fedora's review guidelines treat that as "binary repackaging" and COPR
projects with synthetic RPMs are sometimes flagged. Building from the
spec via `rpmbuild` in a mock chroot produces an RPM indistinguishable
from a "native" Fedora package, which is the path Fedora reviewers
expect if we later push to upstream Fedora.
