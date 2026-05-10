#!/usr/bin/env bash
# Local development install.
#
# Usage:
#   cargo build --release                          # as your normal user
#   sudo bash install-dev.sh                       # as root
#   sudo bash install-dev.sh --skip-models         # skip face model download
#   sudo bash install-dev.sh --skip-ort            # skip ONNX Runtime download
set -euo pipefail

ORT_VERSION="1.24.2"
DETECTOR_SHA256="5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91"
EMBEDDER_SHA256="4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43"

# ── Helpers ───────────────────────────────────────────────────────────────────

http_get() {
  local url="$1" dest="$2"
  if command -v curl &>/dev/null; then
    curl -L --progress-bar -o "$dest" "$url"
  elif command -v wget &>/dev/null; then
    wget --show-progress -q -O "$dest" "$url"
  else
    echo "Error: neither curl nor wget found. Install one and rerun." >&2
    return 1
  fi
}

verify_sha256() {
  local file="$1" expected="$2"
  if ! command -v sha256sum &>/dev/null; then
    echo "Error: sha256sum not found; cannot verify $file" >&2
    return 1
  fi
  local actual
  actual="$(sha256sum "$file" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "Error: checksum mismatch for $file" >&2
    echo "       expected: $expected" >&2
    echo "       actual  : $actual" >&2
    return 1
  fi
}

download_ort() {
  local arch
  case "$(uname -m)" in
    x86_64)  arch="x64" ;;
    aarch64) arch="aarch64" ;;
    *)
      echo "Warning: unsupported architecture $(uname -m) — skipping ORT download." >&2
      echo "         Install libonnxruntime.so manually and rerun." >&2
      return 0
      ;;
  esac

  local name="onnxruntime-linux-${arch}-${ORT_VERSION}"
  local url="https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/${name}.tgz"
  local tmp
  tmp="$(mktemp -d /tmp/facegate-ort-XXXXXX)"

  echo "    Source : $url"
  echo "    Size   : ~10 MB"
  echo ""

  http_get "$url" "$tmp/ort.tgz"

  echo ""
  echo "    Extracting..."
  tar -xzf "$tmp/ort.tgz" -C "$tmp"

  echo "    Installing to /usr/lib/..."
  install -Dm755 "$tmp/$name/lib/libonnxruntime.so.${ORT_VERSION}" \
    "/usr/lib/libonnxruntime.so.${ORT_VERSION}"
  install -Dm755 "$tmp/$name/lib/libonnxruntime_providers_shared.so" \
    /usr/lib/libonnxruntime_providers_shared.so
  ln -sf "libonnxruntime.so.${ORT_VERSION}" /usr/lib/libonnxruntime.so.1
  ln -sf "libonnxruntime.so.1" /usr/lib/libonnxruntime.so

  ldconfig
  rm -rf "$tmp"

  echo "    ONNX Runtime ${ORT_VERSION} installed."
}

download_models() {
  local models_dir="$1"
  local detector="$models_dir/scrfd_500m.onnx"
  local embedder="$models_dir/arcface_w600k_r50.onnx"

  local url="https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"
  local tmp_zip
  tmp_zip="$(mktemp /tmp/facegate-models-XXXXXX.zip)"

  echo "    Source : $url"
  echo "    Size   : ~400 MB"
  echo ""

  http_get "$url" "$tmp_zip"

  echo ""
  echo "    Extracting ONNX models..."
  unzip -jo "$tmp_zip" "*.onnx"   -d "$models_dir" 2>/dev/null || \
  unzip -jo "$tmp_zip" "*/*.onnx" -d "$models_dir" 2>/dev/null || true

  rm -f "$tmp_zip"

  for src in det_10g det_500m; do
    [[ -f "$models_dir/${src}.onnx" ]] && mv "$models_dir/${src}.onnx" "$detector" && break
  done
  for src in w600k_r50 w600k_mbf; do
    [[ -f "$models_dir/${src}.onnx" ]] && mv "$models_dir/${src}.onnx" "$embedder" && break
  done

  if [[ -f "$detector" && -f "$embedder" ]]; then
    verify_sha256 "$detector" "$DETECTOR_SHA256"
    verify_sha256 "$embedder" "$EMBEDDER_SHA256"
    echo "    Detector : $detector  ($(du -sh "$detector" | cut -f1))"
    echo "    Embedder : $embedder  ($(du -sh "$embedder" | cut -f1))"
  else
    echo ""
    echo "Warning: expected ONNX files not found in the archive." >&2
    echo "         Files in $models_dir:" >&2
    ls "$models_dir" >&2 || true
    echo "         Update [models] in /etc/facegate/config.toml to match." >&2
  fi
}

ort_present() {
  ldconfig -p 2>/dev/null | grep -q libonnxruntime && return 0
  find /usr/lib /usr/local/lib -maxdepth 1 -name 'libonnxruntime.so*' \
    -print -quit 2>/dev/null | grep -q .
}

# ── Parse args ────────────────────────────────────────────────────────────────

SKIP_MODELS=0
SKIP_ORT=0
for arg in "$@"; do
  [[ "$arg" == "--skip-models" ]] && SKIP_MODELS=1
  [[ "$arg" == "--skip-ort"    ]] && SKIP_ORT=1
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ── Build step (run as normal user) ──────────────────────────────────────────
if [[ $EUID -ne 0 ]]; then
  echo "==> Building release binaries (running as $(whoami))..."
  cd "$SCRIPT_DIR"
  cargo build --release
  echo ""
  echo "Build done. Now run: sudo bash install-dev.sh"
  exit 0
fi

# ── Install step (root required) ─────────────────────────────────────────────
cd "$SCRIPT_DIR"

if [[ ! -f target/release/facegate ]]; then
  echo "Error: target/release/facegate not found." >&2
  echo "       Run 'cargo build --release' first (as your normal user)." >&2
  exit 1
fi

echo "==> Installing CLI..."
install -Dm755 target/release/facegate           /usr/bin/facegate

echo "==> Installing PAM module..."
install -Dm755 target/release/libpam_facegate.so /usr/lib/security/pam_facegate.so

echo "==> Creating directories..."
mkdir -p /etc/facegate
mkdir -p /usr/share/facegate/models
mkdir -p /var/lib/facegate/users

echo "==> Installing config..."
if [[ ! -f /etc/facegate/config.toml ]]; then
  install -Dm644 config.example.toml /etc/facegate/config.toml
  echo "    Installed /etc/facegate/config.toml (edit to set your camera device)"
else
  echo "    /etc/facegate/config.toml already exists, skipping."
fi

echo "==> Installing shell completions..."
mkdir -p /usr/share/zsh/site-functions
/usr/bin/facegate completions zsh  > /usr/share/zsh/site-functions/_facegate
mkdir -p /usr/share/bash-completion/completions
/usr/bin/facegate completions bash > /usr/share/bash-completion/completions/facegate
mkdir -p /usr/share/fish/vendor_completions.d
/usr/bin/facegate completions fish > /usr/share/fish/vendor_completions.d/facegate.fish

echo "==> Installing man page..."
install -Dm644 docs/facegate.1 /usr/share/man/man1/facegate.1

echo "==> Installing systemd user service..."
install -Dm644 systemd/facegate-watch.service /usr/lib/systemd/user/facegate-watch.service

# ── ONNX Runtime ──────────────────────────────────────────────────────────────
if [[ $SKIP_ORT -eq 1 ]]; then
  echo "==> Skipping ONNX Runtime download (--skip-ort)."
elif ort_present; then
  echo "==> ONNX Runtime already present, skipping download."
else
  echo "==> Downloading ONNX Runtime ${ORT_VERSION}..."
  download_ort
fi

# ── Face recognition models ───────────────────────────────────────────────────
MODELS_DIR="/usr/share/facegate/models"
DETECTOR="$MODELS_DIR/scrfd_500m.onnx"
EMBEDDER="$MODELS_DIR/arcface_w600k_r50.onnx"

if [[ $SKIP_MODELS -eq 1 ]]; then
  echo "==> Skipping model download (--skip-models)."
elif [[ -f "$DETECTOR" && -f "$EMBEDDER" ]]; then
  echo "==> Face models already present, skipping download."
else
  echo "==> Downloading face recognition models..."
  download_models "$MODELS_DIR"
fi

# ── Permissions ───────────────────────────────────────────────────────────────
echo "==> Fixing permissions..."
chown -R root:root /etc/facegate /usr/share/facegate /var/lib/facegate
chmod 755 /var/lib/facegate /var/lib/facegate/users
chmod 644 /etc/facegate/config.toml
chmod 644 "$MODELS_DIR"/*.onnx 2>/dev/null || true

echo ""
echo "Installation complete."
echo ""
echo "Next steps:"
echo "  1. Find IR vs RGB:   facegate cameras   (no root needed)"
echo "  2. Set your camera:  sudo facegate configure   (prefer the IR one)"
echo "  3. Check everything: sudo facegate doctor"
echo "  4. Enroll your face: sudo facegate add \$USER --for both"
echo "  5. Test:             sudo facegate test \$USER"
echo "  6. Enable screen-lock daemon (as your normal user):"
echo "     systemctl --user enable --now facegate-watch"
