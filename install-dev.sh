#!/usr/bin/env bash
# Local development install.
#
# Usage:
#   cargo build --release               # as your normal user
#   sudo bash install-dev.sh            # as root (downloads models automatically)
#   sudo bash install-dev.sh --skip-models   # skip model download
set -euo pipefail

# ── Helpers ───────────────────────────────────────────────────────────────────

download_models() {
  local models_dir="$1"
  local detector="$models_dir/scrfd_500m.onnx"
  local embedder="$models_dir/arcface_w600k_r50.onnx"

  local url="https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"
  local tmp_zip
  tmp_zip="$(mktemp /tmp/face-rs-models-XXXXXX.zip)"

  echo "    Source : $url"
  echo "    Size   : ~400 MB"
  echo ""

  if command -v curl &>/dev/null; then
    curl -L --progress-bar -o "$tmp_zip" "$url"
  elif command -v wget &>/dev/null; then
    wget --show-progress -q -O "$tmp_zip" "$url"
  else
    echo "Error: neither curl nor wget found. Install one and rerun." >&2
    rm -f "$tmp_zip"
    return 1
  fi

  echo ""
  echo "    Extracting ONNX models..."
  # -j junk directory paths, -o overwrite, -n skip if exists
  unzip -jop "$tmp_zip" "*.onnx" > /dev/null 2>&1 || true
  unzip -jo  "$tmp_zip" "*.onnx"    -d "$models_dir" 2>/dev/null || \
  unzip -jo  "$tmp_zip" "*/*.onnx"  -d "$models_dir" 2>/dev/null || true

  rm -f "$tmp_zip"

  # The buffalo_l pack contains det_10g.onnx + w600k_r50.onnx
  # buffalo_sc contains det_500m.onnx + w600k_mbf.onnx
  # Rename whichever we got to the names expected by config.toml
  for src in det_10g det_500m; do
    [[ -f "$models_dir/${src}.onnx" ]] && mv "$models_dir/${src}.onnx" "$detector" && break
  done
  for src in w600k_r50 w600k_mbf; do
    [[ -f "$models_dir/${src}.onnx" ]] && mv "$models_dir/${src}.onnx" "$embedder" && break
  done

  if [[ -f "$detector" && -f "$embedder" ]]; then
    echo "    Detector : $detector  ($(du -sh "$detector" | cut -f1))"
    echo "    Embedder : $embedder  ($(du -sh "$embedder" | cut -f1))"
  else
    echo ""
    echo "Warning: expected ONNX files not found in the archive." >&2
    echo "         Files in $models_dir:" >&2
    ls "$models_dir" >&2 || true
    echo ""
    echo "         Update [models] in /etc/face-rs/config.toml to match." >&2
  fi
}

# ── Parse args ────────────────────────────────────────────────────────────────

SKIP_MODELS=0
for arg in "$@"; do
  [[ "$arg" == "--skip-models" ]] && SKIP_MODELS=1
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

if [[ ! -f target/release/face-rs ]]; then
  echo "Error: target/release/face-rs not found." >&2
  echo "       Run 'cargo build --release' first (as your normal user)." >&2
  exit 1
fi

echo "==> Installing CLI..."
install -Dm755 target/release/face-rs           /usr/bin/face-rs

echo "==> Installing PAM module..."
install -Dm755 target/release/libpam_face_rs.so /usr/lib/security/pam_face_rs.so

echo "==> Creating directories..."
mkdir -p /etc/face-rs
mkdir -p /usr/share/face-rs/models
mkdir -p /var/lib/face-rs/users

echo "==> Installing config..."
if [[ ! -f /etc/face-rs/config.toml ]]; then
  install -Dm644 config.example.toml /etc/face-rs/config.toml
  echo "    Installed /etc/face-rs/config.toml (edit to set your camera device)"
else
  echo "    /etc/face-rs/config.toml already exists, skipping."
fi

echo "==> Installing shell completions..."
mkdir -p /usr/share/zsh/site-functions
/usr/bin/face-rs completions zsh  > /usr/share/zsh/site-functions/_face-rs
mkdir -p /usr/share/bash-completion/completions
/usr/bin/face-rs completions bash > /usr/share/bash-completion/completions/face-rs
mkdir -p /usr/share/fish/vendor_completions.d
/usr/bin/face-rs completions fish > /usr/share/fish/vendor_completions.d/face-rs.fish

# ── Models ────────────────────────────────────────────────────────────────────
MODELS_DIR="/usr/share/face-rs/models"
DETECTOR="$MODELS_DIR/scrfd_500m.onnx"
EMBEDDER="$MODELS_DIR/arcface_w600k_r50.onnx"

if [[ $SKIP_MODELS -eq 1 ]]; then
  echo "==> Skipping model download (--skip-models)."
elif [[ -f "$DETECTOR" && -f "$EMBEDDER" ]]; then
  echo "==> Models already present, skipping download."
else
  echo "==> Downloading face recognition models..."
  download_models "$MODELS_DIR"
fi

# ── Permissions ───────────────────────────────────────────────────────────────
echo "==> Fixing permissions..."
chown -R root:root /etc/face-rs /usr/share/face-rs /var/lib/face-rs
chmod 755 /var/lib/face-rs /var/lib/face-rs/users
chmod 644 /etc/face-rs/config.toml
chmod 644 "$MODELS_DIR"/*.onnx 2>/dev/null || true

echo ""
echo "Installation complete."
echo ""
echo "Next steps:"
echo "  1. Set your camera:   v4l2-ctl --list-devices  →  sudo face-rs configure"
echo "  2. Check everything:  sudo face-rs doctor"
echo "  3. Enroll your face:  sudo face-rs add \$USER --label normal"
echo "  4. Test:              sudo face-rs test \$USER"
