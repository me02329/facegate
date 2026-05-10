#!/usr/bin/env bash
set -euo pipefail

ORT_VERSION="1.24.2"
MODELS_DIR="/usr/share/facegate/models"
DETECTOR="$MODELS_DIR/scrfd_500m.onnx"
EMBEDDER="$MODELS_DIR/arcface_w600k_r50.onnx"
DETECTOR_SHA256="5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91"
EMBEDDER_SHA256="4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43"

# ── Directories + permissions ─────────────────────────────────────────────────
mkdir -p /etc/facegate "$MODELS_DIR" /var/lib/facegate/users
chown -R root:root /etc/facegate /usr/share/facegate /var/lib/facegate
chmod 755 /var/lib/facegate /var/lib/facegate/users
chmod 644 /etc/facegate/config.toml 2>/dev/null || true

# ── Shell completions ─────────────────────────────────────────────────────────
mkdir -p /usr/share/zsh/site-functions
facegate completions zsh  > /usr/share/zsh/site-functions/_facegate        2>/dev/null || true
mkdir -p /usr/share/bash-completion/completions
facegate completions bash > /usr/share/bash-completion/completions/facegate 2>/dev/null || true
mkdir -p /usr/share/fish/vendor_completions.d
facegate completions fish > /usr/share/fish/vendor_completions.d/facegate.fish 2>/dev/null || true

# ── systemd user service ──────────────────────────────────────────────────────
# Reload the system daemon so the new unit file is visible.
# User instances pick it up automatically at next login.
systemctl daemon-reload 2>/dev/null || true

# ── Helpers ───────────────────────────────────────────────────────────────────

# Returns 0 if both stdin and stdout are connected to a terminal.
is_interactive() { [ -t 0 ] && [ -t 1 ]; }

ask_yn() {
    local prompt="$1"
    local yn
    printf "\n%s [Y/n] " "$prompt"
    read -r yn </dev/tty || { echo; return 1; }
    case "${yn:-y}" in
        [Yy]*|"") return 0 ;;
        *) return 1 ;;
    esac
}

http_get() {
    local url="$1" dest="$2"
    if command -v curl &>/dev/null; then
        curl -L --progress-bar -o "$dest" "$url"
    elif command -v wget &>/dev/null; then
        wget --show-progress -q -O "$dest" "$url"
    else
        echo "Error: neither curl nor wget found. Install one and retry." >&2
        return 1
    fi
}

verify_sha256() {
    local file="$1" expected="$2"
    if ! command -v sha256sum &>/dev/null; then
        echo "Warning: sha256sum not found, skipping checksum verification." >&2
        return 0
    fi
    local actual
    actual="$(sha256sum "$file" | awk '{print $1}')"
    if [ "$actual" != "$expected" ]; then
        echo "Error: checksum mismatch for $(basename "$file")" >&2
        echo "  expected: $expected" >&2
        echo "  actual:   $actual" >&2
        return 1
    fi
}

ort_present() {
    ldconfig -p 2>/dev/null | grep -q libonnxruntime && return 0
    find /usr/lib /usr/local/lib -maxdepth 1 -name 'libonnxruntime.so*' \
        -print -quit 2>/dev/null | grep -q .
}

models_present() { [ -f "$DETECTOR" ] && [ -f "$EMBEDDER" ]; }

# ── Install ONNX Runtime ──────────────────────────────────────────────────────

install_ort() {
    local arch
    case "$(uname -m)" in
        x86_64)  arch="x64" ;;
        aarch64) arch="aarch64" ;;
        *)
            echo "Warning: unsupported architecture $(uname -m) — install ONNX Runtime manually." >&2
            return 1
            ;;
    esac

    local name="onnxruntime-linux-${arch}-${ORT_VERSION}"
    local url="https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/${name}.tgz"
    local tmp
    tmp="$(mktemp -d /tmp/facegate-ort-XXXXXX)"

    echo "Downloading ONNX Runtime ${ORT_VERSION} (~10 MB)…"
    http_get "$url" "$tmp/ort.tgz"
    echo "Extracting…"
    tar -xzf "$tmp/ort.tgz" -C "$tmp"

    install -Dm755 "$tmp/$name/lib/libonnxruntime.so.${ORT_VERSION}" \
        "/usr/lib/libonnxruntime.so.${ORT_VERSION}"
    install -Dm755 "$tmp/$name/lib/libonnxruntime_providers_shared.so" \
        /usr/lib/libonnxruntime_providers_shared.so
    ln -sf "libonnxruntime.so.${ORT_VERSION}" /usr/lib/libonnxruntime.so.1
    ln -sf "libonnxruntime.so.1"              /usr/lib/libonnxruntime.so
    ldconfig
    rm -rf "$tmp"
    echo "✓ ONNX Runtime ${ORT_VERSION} installed."
}

# ── Install face models ───────────────────────────────────────────────────────

install_models() {
    local url="https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"
    local tmp_zip
    tmp_zip="$(mktemp /tmp/facegate-models-XXXXXX.zip)"

    echo "Downloading face recognition models (~400 MB)…"
    http_get "$url" "$tmp_zip"
    echo "Extracting…"
    unzip -jo "$tmp_zip" "*.onnx"   -d "$MODELS_DIR" 2>/dev/null || \
    unzip -jo "$tmp_zip" "*/*.onnx" -d "$MODELS_DIR" 2>/dev/null || true
    rm -f "$tmp_zip"

    for src in det_10g det_500m; do
        [ -f "$MODELS_DIR/${src}.onnx" ] && mv "$MODELS_DIR/${src}.onnx" "$DETECTOR" && break
    done
    for src in w600k_r50 w600k_mbf; do
        [ -f "$MODELS_DIR/${src}.onnx" ] && mv "$MODELS_DIR/${src}.onnx" "$EMBEDDER" && break
    done

    if models_present; then
        verify_sha256 "$DETECTOR" "$DETECTOR_SHA256"
        verify_sha256 "$EMBEDDER" "$EMBEDDER_SHA256"
        echo "✓ Face models installed."
    else
        echo "Warning: expected model files not found in the archive." >&2
        echo "  Run 'sudo facegate doctor' for details." >&2
    fi
}

# ── ONNX Runtime prompt ───────────────────────────────────────────────────────
echo ""
if ort_present; then
    echo "✓ ONNX Runtime already installed."
elif is_interactive; then
    if ask_yn "Download and install ONNX Runtime ${ORT_VERSION}? (~10 MB)"; then
        install_ort
    else
        echo "Skipped. Run 'sudo facegate doctor' for manual install instructions."
    fi
else
    cat <<'MSG'
Note: ONNX Runtime is required but was not found.
  Install it from your package manager (e.g. onnxruntime, libonnxruntime) or
  run 'sudo facegate doctor' after installation for step-by-step instructions.
MSG
fi

# ── Models — always download (required, no distro package available) ──────────
echo ""
if models_present; then
    echo "✓ Face recognition models already installed."
else
    if is_interactive; then
        ask_yn "Download face recognition models? (~400 MB)" || {
            echo "Skipped. Run 'sudo facegate doctor' for manual install instructions."
            models_skipped=1
        }
    fi
    if [ "${models_skipped:-0}" = "0" ]; then
        install_models
    fi
fi

# ── Next steps ────────────────────────────────────────────────────────────────
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " Facegate installed. Next steps:"
echo ""
echo "  1. facegate cameras                  # find IR vs RGB cameras"
echo "  2. sudo facegate configure           # set [camera].device"
echo "  3. sudo facegate doctor"
echo "  4. sudo facegate add \$USER --for both"
echo "  5. sudo facegate session-auth"
echo "  6. systemctl --user enable --now facegate-watch"
echo ""
echo " On laptops with Windows-Hello hardware the IR camera (often"
echo " /dev/video2) is preferred over the RGB webcam — it works in"
echo " the dark and resists photo spoofing. 'facegate cameras' will"
echo " tell you which device on this machine is which."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
