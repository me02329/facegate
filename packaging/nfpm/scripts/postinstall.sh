#!/usr/bin/env bash
# Postinstall script for facegate packages (.deb / .rpm / .pkg.tar.zst).
#
# Runs as root via the package manager. Idempotent on upgrades. Closes the
# punch-list tracked in issue #13:
#   - strict shell flags + restrictive umask;
#   - atomic, race-free creation of /var/lib/facegate/audit.log;
#   - hard-fail on missing/mismatched SHA256;
#   - --fail on curl, no silent HTML-as-tarball;
#   - explicit unzip availability check;
#   - try-restart of facegate-brokerd on upgrade;
#   - distinguishes "systemd absent" from "systemd refused";
#   - interactive default flipped to "no" for large downloads;
#   - migrate_template_storage takes exclusive control before traversal.
set -euo pipefail
umask 077

ORT_VERSION="1.24.2"
MODELS_DIR="/usr/share/facegate/models"
DETECTOR="$MODELS_DIR/scrfd_500m.onnx"
EMBEDDER="$MODELS_DIR/arcface_w600k_r50.onnx"
DETECTOR_SHA256="5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91"
EMBEDDER_SHA256="4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43"

# ── facegate system user and group ────────────────────────────────────────────

if ! getent group facegate >/dev/null; then
    groupadd --system facegate
fi
if [ -x /usr/bin/nologin ]; then
    facegate_nologin=/usr/bin/nologin
elif [ -x /usr/sbin/nologin ]; then
    facegate_nologin=/usr/sbin/nologin
elif [ -x /sbin/nologin ]; then
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
# With set -e, a failed useradd above already aborts. Belt-and-braces: make
# sure the broker's uid actually exists before we start chowning things.
if ! id facegate >/dev/null 2>&1; then
    echo "Error: 'facegate' system user is missing after useradd; aborting." >&2
    exit 1
fi

# ── Directory layout ──────────────────────────────────────────────────────────

mkdir -p /etc/facegate "$MODELS_DIR" /var/lib/facegate
chown -R root:root /etc/facegate /usr/share/facegate
chmod 755 /etc/facegate /usr/share/facegate

chown root:root /var/lib/facegate
chmod 755 /var/lib/facegate

# ── Template storage migration (v0.1.0 → v0.2.0) ──────────────────────────────

migrate_template_storage() {
    local users_dir="/var/lib/facegate/users"

    if [ -L "$users_dir" ]; then
        echo "Warning: $users_dir is a symlink; leaving template storage untouched." >&2
        return 0
    fi

    if [ ! -d "$users_dir" ]; then
        install -d -m 0700 -o facegate -g facegate "$users_dir"
        return 0
    fi

    # Take exclusive control of the top-level dir BEFORE walking the tree.
    # After this point, the previous owner (if it was the enrolled user) can
    # no longer create new entries directly under users_dir.
    chown -h facegate:facegate "$users_dir"
    chmod 700 "$users_dir"

    # Refuse to migrate if any suspicious file types exist anywhere in the
    # tree. -P keeps find from following symlinks; we report the symlinks
    # themselves rather than their targets.
    local suspicious
    suspicious="$(find -P "$users_dir" -mindepth 1 \
        \( -type l -o -type p -o -type s -o -type b -o -type c \) -print 2>/dev/null || true)"
    if [ -n "$suspicious" ]; then
        echo "Warning: suspicious template paths under $users_dir; refusing to migrate." >&2
        echo "$suspicious" >&2
        echo "Inspect them manually, then rerun:" >&2
        echo "  sudo facegate broker repair-permissions" >&2
        return 0
    fi

    # Walk subdirectories and embeddings.json files only. -h on chown is
    # belt-and-braces — find -P + the explicit -type filters already mean we
    # never see a symlink here, but if a race injects one we still won't
    # dereference it.
    find -P "$users_dir" -mindepth 1 -type d \
        -exec chown -h facegate:facegate {} + \
        -exec chmod 700 {} +
    find -P "$users_dir" -mindepth 1 -type f -name embeddings.json \
        -exec chown -h facegate:facegate {} + \
        -exec chmod 600 {} +
}
migrate_template_storage

# ── Audit log: atomic create with the right ownership ─────────────────────────

# On a fresh install, install(1) creates the file with the requested mode
# and ownership in a single step — no root:root 0644 window. On upgrade,
# the file already exists and we just enforce its perms.
if [ ! -e /var/lib/facegate/audit.log ]; then
    install -m 0600 -o facegate -g facegate /dev/null /var/lib/facegate/audit.log
else
    chown facegate:facegate /var/lib/facegate/audit.log
    chmod 600 /var/lib/facegate/audit.log
fi

chmod 644 /etc/facegate/config.toml 2>/dev/null || true

# ── Shell completions ─────────────────────────────────────────────────────────

mkdir -p /usr/share/zsh/site-functions
mkdir -p /usr/share/bash-completion/completions
mkdir -p /usr/share/fish/vendor_completions.d
# A broken binary at install time shouldn't fail the whole package; the user
# can regenerate completions later via `facegate completions`.
facegate completions zsh  > /usr/share/zsh/site-functions/_facegate            2>/dev/null || true
facegate completions bash > /usr/share/bash-completion/completions/facegate    2>/dev/null || true
facegate completions fish > /usr/share/fish/vendor_completions.d/facegate.fish 2>/dev/null || true

# ── systemd service activation ────────────────────────────────────────────────

# We only invoke systemctl when systemd is actually running. In a chroot or
# a non-systemd container, "systemctl daemon-reload" would either fail or
# do nothing useful — but in a real systemd environment, errors should
# NOT be silently swallowed.
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
    if ! systemctl is-enabled --quiet facegate-brokerd.service 2>/dev/null; then
        systemctl enable facegate-brokerd.service
    fi
    if systemctl is-active --quiet facegate-brokerd.service; then
        # Upgrade path: pick up the new binary without forcing a reboot.
        systemctl try-restart facegate-brokerd.service
    else
        systemctl start facegate-brokerd.service
    fi
else
    cat <<'MSG'
Note: systemd not detected; skipping facegate-brokerd activation.
      Start the broker manually before using facegate auth/watch:
          /usr/bin/facegate-brokerd
MSG
fi

# ── Helpers ───────────────────────────────────────────────────────────────────

is_interactive() { [ -t 0 ] && [ -t 1 ]; }

ask_yn() {
    local prompt="$1"
    local yn
    printf "\n%s [y/N] " "$prompt"
    # Read from /dev/tty so the prompt works even when stdin is piped (e.g.
    # apt-get under sudo). EOF / Ctrl-D defaults to NO — never accidentally
    # trigger a multi-hundred-megabyte download.
    if ! read -r yn </dev/tty; then
        echo
        return 1
    fi
    case "${yn:-n}" in
        [Yy]*) return 0 ;;
        *)     return 1 ;;
    esac
}

http_get() {
    local url="$1" dest="$2"
    if command -v curl >/dev/null 2>&1; then
        # --fail makes curl exit non-zero on HTTP 4xx/5xx instead of saving
        # the error body as a fake archive. --location follows GitHub's
        # release redirects.
        curl --fail --location --progress-bar -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        # wget exits non-zero on HTTP errors by default.
        wget --show-progress --no-verbose -O "$dest" "$url"
    else
        echo "Error: neither curl nor wget found. Install one and retry." >&2
        return 1
    fi
}

verify_sha256() {
    local file="$1" expected="$2"
    if ! command -v sha256sum >/dev/null 2>&1; then
        echo "Error: sha256sum not found; refusing to install unverified $file." >&2
        return 1
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
    if ldconfig -p 2>/dev/null | grep -q libonnxruntime; then
        return 0
    fi
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
    # shellcheck disable=SC2064
    trap "rm -rf '$tmp'" RETURN

    echo "Downloading ONNX Runtime ${ORT_VERSION} (~10 MB)…"
    if ! http_get "$url" "$tmp/ort.tgz"; then
        echo "Error: ONNX Runtime download failed." >&2
        return 1
    fi
    echo "Extracting…"
    tar -xzf "$tmp/ort.tgz" -C "$tmp"
    install -Dm755 "$tmp/$name/lib/libonnxruntime.so.${ORT_VERSION}" \
        "/usr/lib/libonnxruntime.so.${ORT_VERSION}"
    install -Dm755 "$tmp/$name/lib/libonnxruntime_providers_shared.so" \
        /usr/lib/libonnxruntime_providers_shared.so
    ln -sf "libonnxruntime.so.${ORT_VERSION}" /usr/lib/libonnxruntime.so.1
    ln -sf "libonnxruntime.so.1" /usr/lib/libonnxruntime.so
    ldconfig
    echo "✓ ONNX Runtime ${ORT_VERSION} installed."
}

# ── Install face models ───────────────────────────────────────────────────────

install_models() {
    if ! command -v unzip >/dev/null 2>&1; then
        echo "Error: 'unzip' is not installed; cannot extract face models." >&2
        echo "       Install it (e.g. 'apt install unzip' / 'dnf install unzip')" >&2
        echo "       and rerun 'sudo facegate doctor' for the model setup." >&2
        return 1
    fi

    local url="https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"
    local tmp_zip
    tmp_zip="$(mktemp /tmp/facegate-models-XXXXXX.zip)"
    # shellcheck disable=SC2064
    trap "rm -f '$tmp_zip'" RETURN

    echo "Downloading face recognition models (~400 MB)…"
    if ! http_get "$url" "$tmp_zip"; then
        echo "Error: model download failed." >&2
        return 1
    fi
    echo "Extracting…"
    # -j flattens paths inside the archive, so any directory traversal
    # entries are neutralised on extraction.
    unzip -jo "$tmp_zip" "*.onnx"   -d "$MODELS_DIR" 2>/dev/null \
        || unzip -jo "$tmp_zip" "*/*.onnx" -d "$MODELS_DIR" 2>/dev/null \
        || true

    for src in det_10g det_500m; do
        if [ -f "$MODELS_DIR/${src}.onnx" ]; then
            mv "$MODELS_DIR/${src}.onnx" "$DETECTOR"
            break
        fi
    done
    for src in w600k_r50 w600k_mbf; do
        if [ -f "$MODELS_DIR/${src}.onnx" ]; then
            mv "$MODELS_DIR/${src}.onnx" "$EMBEDDER"
            break
        fi
    done

    if ! models_present; then
        echo "Error: expected ONNX files not found in the archive." >&2
        echo "       Run 'sudo facegate doctor' for diagnostics." >&2
        return 1
    fi

    # Hard-fail on checksum mismatch — these models gate authentication.
    verify_sha256 "$DETECTOR" "$DETECTOR_SHA256"
    verify_sha256 "$EMBEDDER" "$EMBEDDER_SHA256"
    chmod 644 "$DETECTOR" "$EMBEDDER"
    echo "✓ Face models installed and verified."
}

# ── ONNX Runtime prompt ───────────────────────────────────────────────────────

echo ""
if ort_present; then
    echo "✓ ONNX Runtime already installed."
elif is_interactive; then
    if ask_yn "Download and install ONNX Runtime ${ORT_VERSION}? (~10 MB)"; then
        install_ort || echo "Warning: ONNX Runtime install failed; see 'sudo facegate doctor'." >&2
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

# ── Models — interactive default is now "no" ──────────────────────────────────

echo ""
if models_present; then
    echo "✓ Face recognition models already installed."
elif is_interactive; then
    if ask_yn "Download face recognition models? (~400 MB)"; then
        install_models || echo "Warning: model install failed; see 'sudo facegate doctor'." >&2
    else
        echo "Skipped. Run 'sudo facegate doctor' for manual install instructions."
    fi
else
    cat <<'MSG'
Note: face recognition models are required but were not found.
  Run 'sudo facegate doctor' after installation for step-by-step instructions
  (we don't auto-download 400 MB during a non-interactive package install).
MSG
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
