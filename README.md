# autovocoder

A Rust auto-vocoder: pitch-detects the input voice, quantizes it to a scale,
synthesizes a carrier (single note, triad, or fixed "Soundwave" pitch), and
drives a classic 16-band channel vocoder. Ships as:

- An **LV2 plugin** with 10 named presets (Ardour on Linux)
- A **standalone CLI** for offline WAV rendering and live audio (Mac / Linux)
- A **WASM** build for running in a webpage via AudioWorklet
- A **shell wrapper** for routing a robot voice into Teams/Zoom/Discord on Mac

## Quick demos

**Intergalactic-style robot voice, live on Mac, into any meeting app:**
```bash
./scripts/robot-voice.sh intergalactic
```

**Vocode a WAV file:**
```bash
autovocoder render --input vocal.wav --output robot.wav --mode fixed --fixed-note 48
```

**In Ardour** (AVLinux or any Linux distro): Mixer → insert → `Autovocoder` →
right-click → pick a preset.

## Install the LV2 plugin (Linux or Mac)

One-shot installer — builds and drops the bundle into `~/.lv2/`:

```bash
./scripts/install-lv2.sh
```

Requires [Rust](https://rustup.rs) (`rustup install stable`). After install,
Ardour / Carla / any LV2 host will see `Autovocoder` after a plugin rescan.

## Standalone CLI

### Build

```bash
# render-only (no audio deps)
cargo build --release -p autovocoder-standalone

# with live mic → speakers support
cargo build --release -p autovocoder-standalone --features live
```

On Linux, `--features live` needs `libasound2-dev` for ALSA.

### Offline render

```bash
./target/release/autovocoder render \
    --input in.wav --output out.wav \
    --mode fixed --fixed-note 48     # Soundwave, C3
```

All modes: `mono` (track the singer's pitch), `major-triad`, `minor-triad`,
`fixed`. Scale snapping: `--scale major --scale-root 0` etc.

Full flag list: `./target/release/autovocoder render --help`.

### Live (mic → speakers)

```bash
./target/release/autovocoder live --mode fixed --fixed-note 48
```

Use headphones — live output into the same mic creates feedback loops fast.

## Robot voice in Teams/Zoom/Discord (Mac)

One-time setup:

1. `brew install blackhole-2ch` — virtual audio device. Reboot after, or:
   `sudo killall coreaudiod`.
2. In **Audio MIDI Setup**, create a Multi-Output Device with both BlackHole
   and your headphones checked — lets you monitor yourself.
3. In Teams / Zoom / Discord, set **Microphone** → `BlackHole 2ch`.

Per-call:

```bash
./scripts/robot-voice.sh intergalactic
# or: ./scripts/robot-voice.sh soundwave-c2
# or: ./scripts/robot-voice.sh --list   # see all presets
```

Ctrl-C to restore your normal voice.

## Presets

The LV2 plugin and the `robot-voice.sh` script share a curated preset set:

| Preset             | Flavor                                                        |
|--------------------|---------------------------------------------------------------|
| `soundwave-c2`     | Deep Decepticon. Fixed C2, fully wet.                         |
| `soundwave-c3`     | Neutral Soundwave register. Fixed C3.                         |
| `soundwave-g2`     | Slightly higher Soundwave — more intelligible.                |
| `intergalactic`    | Beastie Boys refrain vibe — fixed A2, bright buzzy carrier.   |
| `chromatic-robot`  | Tracks pitch chromatically with a snappy glide.               |
| `heavy-glide`      | Same as chromatic-robot but with long portamento (dub-style). |
| `in-key-c-major`   | Autotune-to-C-major. Wrong notes become right notes.          |
| `in-key-a-minor`   | Autotune-to-A-minor. Melodic, natural.                        |
| `dark-choir`       | Minor triad per detected note. Eerie pad effect.              |
| `bright-angels`    | Major triad per detected note. Auto-Tune-on-steroids heaven.  |
| `subtle-support`   | 30% wet — layer under a real vocal for harmonic reinforcement.|

## Architecture

Cargo workspace, one DSP core + three frontends:

```
crates/
├── autovocoder-dsp/          # host-agnostic DSP: osc, filter, pitch (YIN),
│                             # scale, 16-band vocoder, top-level processor
├── autovocoder-lv2/          # LV2 plugin wrapper (hand-rolled C FFI)
│   └── lv2/*.ttl             # plugin manifest + presets
├── autovocoder-standalone/   # CLI: render + live (cpal) subcommands
└── autovocoder-wasm/         # wasm-bindgen bindings for browser use
```

Signal chain in one glance:

```
voice ──► YIN pitch detect ──► scale quantize ──► portamento ──► saw carrier(s)
                                                                         │
voice ──► analysis filterbank ──► envelope followers ──► × carrier bands ─┴─► out
```

## Development

```bash
just check        # fmt + clippy + tests
just test         # run tests only
just render ...   # run the CLI render subcommand
just lv2-bundle   # build the LV2 bundle under target/lv2/
just lv2-install  # build + copy to ~/.lv2/
just wasm         # build the WASM package (needs wasm-pack)
```

Or without `just`, any `cargo` command works — e.g. `cargo test --workspace --exclude autovocoder-wasm`.

## License

MIT OR Apache-2.0 — pick whichever you prefer.
