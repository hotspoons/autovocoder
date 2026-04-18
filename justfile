default:
    @just --list

# Build everything (debug, native)
build:
    cargo build --workspace

# Build everything (release, native)
release:
    cargo build --workspace --release

# Run the standalone offline renderer
render *ARGS:
    cargo run --release -p autovocoder-standalone -- render {{ARGS}}

# Alias: `just run` → `just render`
run *ARGS: (render ARGS)

# Run all tests (native)
test:
    cargo test --workspace --exclude autovocoder-wasm

# Lint with clippy (warnings = errors)
lint:
    cargo clippy --workspace --all-targets --exclude autovocoder-wasm -- -D warnings

# Format all code
fmt:
    cargo fmt --all

# Verify formatting without writing
fmt-check:
    cargo fmt --all -- --check

# All the checks CI would run
check: fmt-check lint test

# Build the LV2 plugin bundle at target/lv2/autovocoder.lv2
lv2-bundle: release
    #!/usr/bin/env bash
    set -euo pipefail
    OUT="target/lv2/autovocoder.lv2"
    mkdir -p "$OUT"
    case "$(uname)" in
        Darwin) EXT="dylib"; LIB="libautovocoder_lv2.$EXT" ;;
        Linux)  EXT="so";    LIB="libautovocoder_lv2.$EXT" ;;
        *)      EXT="dll";   LIB="autovocoder_lv2.$EXT"    ;;
    esac
    cp "target/release/$LIB" "$OUT/"
    cp crates/autovocoder-lv2/lv2/manifest.ttl "$OUT/"
    cp crates/autovocoder-lv2/lv2/autovocoder.ttl "$OUT/"
    cp crates/autovocoder-lv2/lv2/presets.ttl "$OUT/"
    echo "Bundle: $OUT"

# Install the LV2 plugin to ~/.lv2 (builds + copies). Delegates to the
# standalone script so people without `just` can install the same way.
lv2-install:
    scripts/install-lv2.sh

# One-time: install zig + cargo-zigbuild + apple-darwin rustup targets.
# Safe to re-run after a devcontainer rebuild.
mac-setup:
    scripts/setup-macos-cross.sh

# Cross-compile the render (WAV-in/WAV-out) CLI for Apple Silicon.
# NOTE: the `live` feature is intentionally excluded — cpal/coreaudio-sys
# bindgens Apple SDK headers at compile time, which cross-compile can't
# satisfy without a real MacOSX.sdk mount. For live audio, build natively
# on the Mac — see `just mac-live-howto`.
mac-arm64:
    cargo zigbuild -p autovocoder-standalone --release --target aarch64-apple-darwin
    @echo "→ target/aarch64-apple-darwin/release/autovocoder (render only)"

# Cross-compile the render CLI for Intel macs.
mac-x86:
    cargo zigbuild -p autovocoder-standalone --release --target x86_64-apple-darwin
    @echo "→ target/x86_64-apple-darwin/release/autovocoder (render only)"

# Universal2 binary (arm64 + x86_64, lipo'd). Works on any Mac. Render only.
mac-universal:
    cargo zigbuild -p autovocoder-standalone --release --target universal2-apple-darwin
    @echo "→ target/universal2-apple-darwin/release/autovocoder (render only)"

# Print the recipe for getting a live-audio binary on your Mac.
mac-live-howto:
    @echo "Live audio (cpal → CoreAudio) can't cross-compile from Linux"
    @echo "without a full Apple SDK. Build on the Mac itself instead:"
    @echo ""
    @echo "  # one-time, on the Mac:"
    @echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    @echo "  # then in a clone of this repo, on the Mac:"
    @echo "  cargo build --release -p autovocoder-standalone --features live"
    @echo "  ./target/release/autovocoder live --mode fixed --fixed-note 48"

# Build the WASM package (needs wasm-pack: `cargo install wasm-pack`)
wasm:
    wasm-pack build crates/autovocoder-wasm --target web --out-dir ../../target/wasm-pkg

# Check that the WASM crate at least compiles for wasm32 (no wasm-pack needed)
wasm-check:
    cargo check -p autovocoder-wasm --target wasm32-unknown-unknown

# Clean build artifacts
clean:
    cargo clean
    rm -rf target/lv2 target/wasm-pkg
