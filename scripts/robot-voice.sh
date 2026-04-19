#!/usr/bin/env bash
# One-command robot voice: run the autovocoder live, routing output to
# a virtual audio device (BlackHole on Mac) so other apps — Teams, Zoom,
# Discord, OBS — pick it up as if it were your microphone.
#
# Usage:
#   ./scripts/robot-voice.sh                     # default preset (soundwave-c3)
#   ./scripts/robot-voice.sh soundwave-c2        # pick a preset
#   ./scripts/robot-voice.sh --list              # show available presets
#   ./scripts/robot-voice.sh soundwave-c3 --mix 0.8    # preset + extra flags
#
# Environment overrides:
#   AUTOVOCODER_BIN    — path to the built binary
#                        (default: ./target/release/autovocoder)
#   OUTPUT_DEVICE      — substring of the virtual audio device name
#                        (default: BlackHole)
#
# This script is for Mac. On Linux, the output-device flag still works
# (route to a PulseAudio null sink or similar) but BlackHole detection is
# skipped.

set -euo pipefail

# --- Config -----------------------------------------------------------------

BIN="${AUTOVOCODER_BIN:-./target/release/autovocoder}"
OUTPUT_DEVICE="${OUTPUT_DEVICE:-BlackHole}"
DEFAULT_PRESET="soundwave-c3"

# Preset → CLI-flags lookup. Mirrors the LV2 presets where it makes sense.
# Authentic Soundwave: fixed-root minor triad at low register (per the
# Scott Brownlee / Frank Welker audio engineering interview).
preset_flags() {
    case "$1" in
        soundwave-c2)     echo "--mode fixed-chord --chord minor --fixed-note 36 --portamento 25 --mix 1.0" ;;
        soundwave-c3)     echo "--mode fixed-chord --chord minor --fixed-note 48 --portamento 25 --mix 1.0" ;;
        soundwave-g2)     echo "--mode fixed-chord --chord minor --fixed-note 43 --portamento 25 --mix 1.0" ;;
        # Intergalactic: fixed A2 single saw, bright carrier, hot input.
        # Try --fixed-note 43/47 to shift register with the track.
        intergalactic)    echo "--mode fixed --fixed-note 45 --portamento 12 --mix 1.0 --carrier-level 0.8 --input-gain 12 --output-gain 9 --comp-threshold -20" ;;
        chromatic-robot)  echo "--mode mono --portamento 8  --mix 1.0" ;;
        heavy-glide)      echo "--mode mono --portamento 200 --mix 1.0" ;;
        in-key-c-major)   echo "--mode mono --scale major --scale-root 0 --portamento 20 --mix 1.0" ;;
        in-key-a-minor)   echo "--mode mono --scale minor --scale-root 9 --portamento 20 --mix 1.0" ;;
        dark-choir)       echo "--mode chord --chord minor --scale minor --scale-root 9 --portamento 40 --mix 1.0" ;;
        bright-angels)    echo "--mode chord --chord major --scale major --scale-root 0 --portamento 30 --mix 1.0" ;;
        jazz-maj7)        echo "--mode chord --chord maj7 --scale major --scale-root 0 --portamento 35 --mix 1.0 --carrier-level 0.45" ;;
        ominous-dim7)     echo "--mode fixed-chord --chord dim7 --fixed-note 40 --portamento 25 --mix 1.0 --carrier-level 0.5" ;;
        power-chord)      echo "--mode fixed-chord --chord power --fixed-note 33 --portamento 20 --mix 1.0 --carrier-level 0.7 --input-gain 12" ;;
        subtle-support)   echo "--mode mono --portamento 15 --mix 0.3 --input-gain 6 --output-gain 3" ;;
        *) return 1 ;;
    esac
}

ALL_PRESETS=(
    soundwave-c2
    soundwave-c3
    soundwave-g2
    intergalactic
    chromatic-robot
    heavy-glide
    in-key-c-major
    in-key-a-minor
    dark-choir
    bright-angels
    jazz-maj7
    ominous-dim7
    power-chord
    subtle-support
)

# --- Helpers ----------------------------------------------------------------

die() {
    echo "error: $*" >&2
    exit 1
}

list_presets() {
    echo "Available presets:"
    for p in "${ALL_PRESETS[@]}"; do
        local flags
        flags=$(preset_flags "$p")
        printf "  %-18s  %s\n" "$p" "$flags"
    done
    echo ""
    echo "Default: $DEFAULT_PRESET"
}

check_binary() {
    if [[ ! -x "$BIN" ]]; then
        cat >&2 <<EOF
error: binary not found or not executable: $BIN

Build it first (on this machine, from the repo root):
  cargo build --release -p autovocoder-standalone --features live
EOF
        exit 1
    fi
}

check_blackhole_mac() {
    [[ "$(uname)" == "Darwin" ]] || return 0

    if system_profiler SPAudioDataType 2>/dev/null | grep -qi "blackhole"; then
        return 0
    fi

    cat >&2 <<EOF
warning: no BlackHole audio device detected on this Mac.

To route this output into Teams/Zoom/Discord as a microphone, install BlackHole:
  brew install blackhole-2ch
(or download the installer from https://existential.audio/blackhole)

Reboot, then in Audio MIDI Setup create a Multi-Output Device that includes
both BlackHole and your headphones so you can still hear yourself.

Proceeding anyway — output will go to whichever device matches
--output-device '$OUTPUT_DEVICE' (or fail if no match).
EOF
}

# --- Main -------------------------------------------------------------------

if [[ $# -gt 0 && ( "$1" == "--list" || "$1" == "-l" ) ]]; then
    list_presets
    exit 0
fi

if [[ $# -gt 0 && ( "$1" == "--help" || "$1" == "-h" ) ]]; then
    sed -n '3,18p' "$0" | sed 's/^# \?//'
    echo ""
    list_presets
    exit 0
fi

preset="${1:-$DEFAULT_PRESET}"
shift || true  # consume preset name if present; any remaining args pass through

if ! flags=$(preset_flags "$preset"); then
    echo "error: unknown preset '$preset'" >&2
    echo "" >&2
    list_presets >&2
    exit 1
fi

check_binary
check_blackhole_mac

# Build the argv we'll exec, then format it as a copy-pasteable command.
# Word-splitting on $flags is intentional — it's a space-joined flag list.
# shellcheck disable=SC2206
cmd=("$BIN" live --output-device "$OUTPUT_DEVICE" $flags "$@")
printf -v cmd_str '%q ' "${cmd[@]}"

cat <<EOF
──────────────────────────────────────────────────────────────
  preset:         $preset
  output device:  $OUTPUT_DEVICE (substring match)

  command:
    ${cmd_str% }

  In Teams/Zoom/Discord, set your MICROPHONE to the BlackHole device.
  Press Ctrl-C here to stop and restore your normal voice.
──────────────────────────────────────────────────────────────
EOF

exec "${cmd[@]}"
