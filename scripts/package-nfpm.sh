#!/usr/bin/env bash
set -euo pipefail

if ! command -v nfpm >/dev/null 2>&1; then
  echo "Error: nfpm is required. Install it from https://nfpm.goreleaser.com/" >&2
  exit 1
fi

version="${FACEGATE_VERSION:-$(git describe --tags --always --dirty 2>/dev/null || echo dev)}"
export FACEGATE_VERSION="$version"

cargo build --release

mkdir -p dist
nfpm package --config packaging/nfpm/facegate.yaml --packager deb --target "dist/facegate_${version}_amd64.deb"
nfpm package --config packaging/nfpm/facegate.yaml --packager rpm --target "dist/facegate-${version}.x86_64.rpm"

echo "Built packages in dist/"
