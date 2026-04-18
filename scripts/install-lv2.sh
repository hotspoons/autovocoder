#!/usr/bin/env bash
# Build the autovocoder LV2 plugin and install it to ~/.lv2/.
#
# Works on Linux (primary target, AVLinux / Ubuntu / Debian / Fedora / Arch)
# and Mac. The only hard requirement is a Rust toolchain via rustup.
#
# Idempotent — re-run to upgrade in place after pulling new changes.
#
#   ./scripts/install-lv2.sh                    # build + install to ~/.lv2
#   INSTALL_DIR=/usr/local/lib/lv2 ./scripts/install-lv2.sh    # system-wide
#   ./scripts/install-lv2.sh --dry-run          # show what would happen
#   ./scripts/install-lv2.sh --uninstall        # remove the installed bundle

set -euo pipefail

BUNDLE_NAME="autovocoder.lv2"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.lv2}"
DRY_RUN=0
UNINSTALL=0

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

die()  { echo "error: $*" >&2; exit 1; }
info() { echo "==> $*"; }
run()  { if [[ "$DRY_RUN" -eq 1 ]]; then echo "   [dry-run] $*"; else "$@"; fi; }

for arg in "$@"; do
    case "$arg" in
        --dry-run)   DRY_RUN=1 ;;
        --uninstall) UNINSTALL=1 ;;
        -h|--help)
            sed -n '3,14p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *) die "unknown option: $arg" ;;
    esac
done

target_bundle="$INSTALL_DIR/$BUNDLE_NAME"

if [[ "$UNINSTALL" -eq 1 ]]; then
    if [[ -d "$target_bundle" ]]; then
        info "Removing $target_bundle"
        run rm -rf "$target_bundle"
        info "Uninstalled. Rescan plugins in your DAW to notice."
    else
        info "Nothing to remove: $target_bundle does not exist."
    fi
    exit 0
fi

# --- Detect platform --------------------------------------------------------

case "$(uname)" in
    Linux)  LIB_EXT="so";    LIB_PREFIX="lib" ;;
    Darwin) LIB_EXT="dylib"; LIB_PREFIX="lib" ;;
    *)      die "unsupported OS: $(uname). LV2 only tested on Linux and Mac." ;;
esac
LIB_NAME="${LIB_PREFIX}autovocoder_lv2.${LIB_EXT}"

# --- Preflight --------------------------------------------------------------

if ! command -v cargo >/dev/null 2>&1; then
    die "cargo not found. Install the Rust toolchain: https://rustup.rs"
fi

cd "$REPO_ROOT"

# --- Build ------------------------------------------------------------------

info "Building release plugin ($LIB_NAME)..."
run cargo build --release -p autovocoder-lv2

built_lib="target/release/$LIB_NAME"
if [[ "$DRY_RUN" -eq 0 && ! -f "$built_lib" ]]; then
    die "build succeeded but $built_lib not found — something is wrong."
fi

# --- Stage + install --------------------------------------------------------

info "Installing to $target_bundle"
run mkdir -p "$target_bundle"
run cp "$built_lib" "$target_bundle/$LIB_NAME"
run cp crates/autovocoder-lv2/lv2/manifest.ttl   "$target_bundle/"
run cp crates/autovocoder-lv2/lv2/autovocoder.ttl "$target_bundle/"
run cp crates/autovocoder-lv2/lv2/presets.ttl    "$target_bundle/"

# If manifest.ttl still points at libautovocoder_lv2.so but we're on Mac,
# patch the binary reference in the installed copy so hosts can find it.
if [[ "$LIB_EXT" != "so" && "$DRY_RUN" -eq 0 ]]; then
    sed -i.bak "s|libautovocoder_lv2\.so|$LIB_NAME|g" "$target_bundle/manifest.ttl"
    rm -f "$target_bundle/manifest.ttl.bak"
fi

cat <<EOF

Installed:   $target_bundle
Contents:
$(ls -1 "$target_bundle" 2>/dev/null | sed 's|^|    |')

Next steps:
  1. Open your DAW (Ardour, Carla, jalv, ...) and rescan plugins.
  2. Look for 'Autovocoder' in the plugin picker.
  3. Right-click the plugin → Presets → pick one (Soundwave, Intergalactic, etc.).

To upgrade later:  git pull && ./scripts/install-lv2.sh
To uninstall:      ./scripts/install-lv2.sh --uninstall
EOF
