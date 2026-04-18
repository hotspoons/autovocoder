#!/usr/bin/env bash
# Set up macOS cross-compilation from a Linux dev container.
#
# What this installs:
#   - zig 0.13.0 at /opt/zig (symlinked to /usr/local/bin/zig)
#   - cargo-zigbuild (via `cargo install`)
#   - rustup targets: aarch64-apple-darwin, x86_64-apple-darwin
#
# Idempotent — safe to re-run. Nothing here is specific to a single
# container instance, so this script lives in the repo and can be re-run
# after a devcontainer rebuild.
#
# Why this approach?
#   Linux→Mac cross-compilation with pure-Rust crates just needs a linker
#   that speaks Mach-O. `cargo-zigbuild` uses zig's built-in Mach-O linker
#   to do exactly that, without an Xcode SDK.
#
#   If we later add crates that actually link Apple frameworks (e.g. cpal
#   → CoreAudio), we'll need to add empty `.framework` stubs under
#   `build/macos-sdk-shim/Frameworks/` and point the linker at them with
#   `-F`. See `/tmp/zip-ties-runner` for a reference implementation.

set -euo pipefail

ZIG_VERSION="0.13.0"

log() { printf '[setup-macos-cross] %s\n' "$*"; }

arch_for_zig() {
    case "$(uname -m)" in
        x86_64)  echo "x86_64" ;;
        aarch64) echo "aarch64" ;;
        arm64)   echo "aarch64" ;;
        *) echo "unsupported host arch: $(uname -m)" >&2; exit 1 ;;
    esac
}

install_zig() {
    if command -v zig >/dev/null 2>&1 && zig version | grep -q "^${ZIG_VERSION}\$"; then
        log "zig ${ZIG_VERSION} already installed"
        return
    fi
    local arch tarball url tmp
    arch="$(arch_for_zig)"
    tarball="zig-linux-${arch}-${ZIG_VERSION}.tar.xz"
    url="https://ziglang.org/download/${ZIG_VERSION}/${tarball}"
    tmp="$(mktemp -d)"
    log "downloading ${url}"
    curl -fsSL "${url}" -o "${tmp}/${tarball}"
    sudo mkdir -p /opt/zig
    sudo tar -C /opt/zig --strip-components=1 -xJf "${tmp}/${tarball}"
    sudo ln -sf /opt/zig/zig /usr/local/bin/zig
    rm -rf "${tmp}"
    log "zig installed: $(zig version)"
}

install_cargo_zigbuild() {
    if command -v cargo-zigbuild >/dev/null 2>&1; then
        log "cargo-zigbuild already installed: $(cargo-zigbuild --version 2>/dev/null | head -1)"
        return
    fi
    log "installing cargo-zigbuild"
    cargo install cargo-zigbuild --locked
}

add_rust_targets() {
    for t in aarch64-apple-darwin x86_64-apple-darwin; do
        if rustup target list --installed | grep -q "^${t}\$"; then
            log "rustup target ${t} already installed"
        else
            log "adding rustup target ${t}"
            rustup target add "${t}"
        fi
    done
}

main() {
    install_zig
    install_cargo_zigbuild
    add_rust_targets
    log "done. Try: just mac-arm64"
}

main "$@"
