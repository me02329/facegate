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

# Default models (since v0.4.0): YuNet for detection (MIT), AuraFace v1 /
# glintr100 for the embedding (Apache 2.0). Both replace the
# InsightFace `buffalo_l` bundle that v0.3.x downloaded under a
# non-commercial pretrained-model licence.
DETECTOR_URL="https://huggingface.co/opencv/face_detection_yunet/resolve/main/face_detection_yunet_2023mar.onnx"
EMBEDDER_URL="https://huggingface.co/fal/AuraFace-v1/resolve/main/glintr100.onnx"
DETECTOR_SHA256="8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4"
# TODO(release): pin the real glintr100.onnx SHA256 before tagging
# v0.4.0; see packaging/nfpm/scripts/postinstall.sh for the same TODO.
EMBEDDER_SHA256="TBD-pin-before-release-see-comment"

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
  local detector="$models_dir/face_detection_yunet_2023mar.onnx"
  local embedder="$models_dir/glintr100.onnx"

  echo "    Detector : YuNet (MIT, ~233 KB)"
  echo "    Embedder : AuraFace v1 / glintr100 (Apache 2.0, ~261 MB)"
  echo ""

  http_get "$DETECTOR_URL" "$detector"
  http_get "$EMBEDDER_URL" "$embedder"

  if [[ -f "$detector" && -f "$embedder" ]]; then
    if ! verify_sha256 "$detector" "$DETECTOR_SHA256"; then
      rm -f "$detector" "$embedder"
      echo "Error: detector checksum mismatch; removed downloads." >&2
      return 1
    fi
    if ! verify_sha256 "$embedder" "$EMBEDDER_SHA256"; then
      rm -f "$detector" "$embedder"
      echo "Error: embedder checksum mismatch; removed downloads." >&2
      return 1
    fi
    echo "    Detector : $detector  ($(du -sh "$detector" | cut -f1))"
    echo "    Embedder : $embedder  ($(du -sh "$embedder" | cut -f1))"
  else
    echo ""
    echo "Warning: expected ONNX files not found after download." >&2
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

echo "==> Installing broker daemon..."
if [[ ! -f target/release/facegate-brokerd ]]; then
  echo "Error: target/release/facegate-brokerd not found." >&2
  echo "       Run 'cargo build --release' first (as your normal user)." >&2
  exit 1
fi
install -Dm755 target/release/facegate-brokerd   /usr/bin/facegate-brokerd

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

echo "==> Installing systemd system service (broker)..."
install -Dm644 systemd/facegate-brokerd.service /usr/lib/systemd/system/facegate-brokerd.service

echo "==> Creating facegate system user and group..."
if ! getent group facegate >/dev/null; then
  groupadd --system facegate
fi
if [[ -x /usr/bin/nologin ]]; then
  facegate_nologin=/usr/bin/nologin
elif [[ -x /usr/sbin/nologin ]]; then
  facegate_nologin=/usr/sbin/nologin
elif [[ -x /sbin/nologin ]]; then
  facegate_nologin=/sbin/nologin
else
  facegate_nologin=/bin/false
fi
if ! id -u facegate >/dev/null 2>&1; then
  useradd \
    --system \
    --no-create-home \
    --home-dir /var/lib/facegate \
    --gid facegate \
    --shell "$facegate_nologin" \
    facegate
fi
if ! id facegate >/dev/null 2>&1; then
  echo "Error: 'facegate' system user is missing after useradd; aborting." >&2
  exit 1
fi

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
DETECTOR="$MODELS_DIR/face_detection_yunet_2023mar.onnx"
EMBEDDER="$MODELS_DIR/glintr100.onnx"

if [[ $SKIP_MODELS -eq 1 ]]; then
  echo "==> Skipping model download (--skip-models)."
elif [[ -f "$DETECTOR" && -f "$EMBEDDER" ]]; then
  echo "==> Face models already present, skipping download."
else
  echo "==> Downloading face recognition models..."
  download_models "$MODELS_DIR"
fi

# ── Permissions and broker-owned storage ─────────────────────────────────────
# /etc and /usr/share belong to root, but /var/lib/facegate must be writable
# by the facegate system user (the broker). This replaces the old
# "chown -R root:root /var/lib/facegate" which fought the broker layout.
echo "==> Fixing permissions and template ownership..."
chown -R root:root /etc/facegate /usr/share/facegate
chown root:root /var/lib/facegate
chmod 755 /var/lib/facegate
chmod 644 /etc/facegate/config.toml
chmod 644 "$MODELS_DIR"/*.onnx 2>/dev/null || true

# Mirror of packaging/nfpm/scripts/postinstall.sh:migrate_template_storage —
# kept inline so this dev script stays self-contained.
migrate_template_storage() {
  local users_dir="/var/lib/facegate/users"
  if [[ -L "$users_dir" ]]; then
    echo "Warning: $users_dir is a symlink; leaving template storage untouched." >&2
    return 0
  fi
  if [[ ! -d "$users_dir" ]]; then
    install -d -m 0700 -o facegate -g facegate "$users_dir"
    return 0
  fi
  chown -h facegate:facegate "$users_dir"
  chmod 700 "$users_dir"
  local suspicious
  suspicious="$(find -P "$users_dir" -mindepth 1 \
    \( -type l -o -type p -o -type s -o -type b -o -type c \) -print 2>/dev/null || true)"
  if [[ -n "$suspicious" ]]; then
    echo "Warning: suspicious template paths under $users_dir; refusing to migrate." >&2
    echo "$suspicious" >&2
    return 0
  fi
  find -P "$users_dir" -mindepth 1 -type d \
    -exec chown -h facegate:facegate {} + \
    -exec chmod 700 {} +
  find -P "$users_dir" -mindepth 1 -type f -name embeddings.json \
    -exec chown -h facegate:facegate {} + \
    -exec chmod 600 {} +
}
migrate_template_storage

# Audit log (broker-owned, atomic on fresh install)
if [[ ! -e /var/lib/facegate/audit.log ]]; then
  install -m 0600 -o facegate -g facegate /dev/null /var/lib/facegate/audit.log
else
  chown facegate:facegate /var/lib/facegate/audit.log
  chmod 600 /var/lib/facegate/audit.log
fi

# ── Activate broker service ──────────────────────────────────────────────────
if [[ -d /run/systemd/system ]] && command -v systemctl >/dev/null 2>&1; then
  echo "==> Activating facegate-brokerd..."
  systemctl daemon-reload
  if ! systemctl is-enabled --quiet facegate-brokerd.service 2>/dev/null; then
    systemctl enable facegate-brokerd.service
  fi
  if systemctl is-active --quiet facegate-brokerd.service; then
    systemctl try-restart facegate-brokerd.service
  else
    systemctl start facegate-brokerd.service
  fi
else
  echo "Note: systemd not detected; skipping facegate-brokerd activation."
  echo "      Start it manually before using face auth: /usr/bin/facegate-brokerd"
fi

echo ""
echo "Installation complete."
echo ""
echo "Next steps:"
echo "  1. Guided setup:     sudo facegate setup"
echo "                       (picks RGB as primary, offers RGB+IR cross-check)"
echo "  2. Verify status:    sudo facegate status"
echo "  3. Check health:     sudo facegate doctor"
echo ""
echo "Manual flow (if you skip setup):"
echo "  - List cameras:        facegate cameras"
echo "  - Configure RGB:       sudo facegate configure"
echo "    (use the RGB device under [camera].device; IR goes in [camera.ir])"
echo "  - Enroll your face:    sudo facegate add \$USER --for both"
echo "  - Calibrate cross-check (if using an IR sensor):"
echo "      sudo facegate calibrate-cameras --write --enable"
echo "  - Test:                sudo facegate test \$USER"
echo "  - Screen-lock daemon (run as your user):"
echo "      systemctl --user enable --now facegate-watch"
