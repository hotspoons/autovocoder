//! Standalone autovocoder: two subcommands.
//!
//!   autovocoder render --input voice.wav --output out.wav [--mode ...]
//!   autovocoder live   [--mode ...]     (requires `live` feature)
//!
//! The `render` path is the offline WAV-in / WAV-out pipeline used for
//! iterating on DSP. The `live` path runs mic → autovocoder → speakers
//! through cpal, for auditioning on a laptop without involving a DAW.

use anyhow::Result;
use autovocoder_dsp::{AutoVocoderConfig, CarrierMode, ChordVoicing, Scale};
use clap::{Args, Parser, Subcommand, ValueEnum};

#[cfg(feature = "live")]
mod live;
mod render;

#[derive(Parser, Debug)]
#[command(name = "autovocoder", about = "Offline render + live audition")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Render a WAV file through the autovocoder.
    Render(render::RenderArgs),
    /// Run the autovocoder on live audio (mic → speakers).
    #[cfg(feature = "live")]
    Live(live::LiveArgs),
}

/// Config knobs shared by both subcommands.
#[derive(Args, Debug, Clone)]
pub struct SharedCfg {
    /// Carrier mode.
    /// - mono:        one saw, tracks detected pitch
    /// - chord:       a chord (see --chord) whose root tracks detected pitch
    /// - fixed:       one saw at --fixed-note
    /// - fixed-chord: a chord rooted at --fixed-note (classic Soundwave)
    #[arg(long, value_enum, default_value_t = Mode::Mono)]
    pub mode: Mode,

    /// Chord voicing for `chord` and `fixed-chord` modes.
    #[arg(long, value_enum, default_value_t = Chord::Major)]
    pub chord: Chord,

    /// Fixed MIDI note for `fixed` / `fixed-chord` modes (default: 48 = C3).
    #[arg(long, default_value_t = 48)]
    pub fixed_note: u8,

    /// Scale to snap detected pitch into.
    #[arg(long, value_enum, default_value_t = ScaleKind::Chromatic)]
    pub scale: ScaleKind,

    /// Root pitch class for non-chromatic scales (0=C, 1=C#, ..., 11=B).
    #[arg(long, default_value_t = 0)]
    pub scale_root: u8,

    /// Dry/wet mix (0.0 voice only, 1.0 vocoded only).
    #[arg(long, default_value_t = 1.0)]
    pub mix: f32,

    /// Portamento (glide) between detected pitches, in ms.
    /// Lower = snappier/more robotic; higher = slurred/dubby.
    #[arg(long, default_value_t = 25.0)]
    pub portamento: f32,

    /// Carrier oscillator level feeding the vocoder (0.0–2.0).
    #[arg(long, default_value_t = 0.6)]
    pub carrier_level: f32,

    /// Input gain in dB, applied to the voice before the vocoder.
    /// Primary knob for loudness — a hotter modulator drives the envelope
    /// followers harder, so the vocoder output is louder before any
    /// post-stage gain. Range -20..+60.
    #[arg(long, default_value_t = 9.0)]
    pub input_gain: f32,

    /// Output makeup gain in dB, applied AFTER the compressor.
    /// Use this to fine-tune the final level after --input-gain has set
    /// the working dynamic range. Range -20..+60.
    #[arg(long, default_value_t = 6.0)]
    pub output_gain: f32,

    /// Disable the built-in output compressor.
    #[arg(long)]
    pub no_compress: bool,

    /// Compressor threshold in dB (default -18).
    #[arg(long, default_value_t = -18.0)]
    pub comp_threshold: f32,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Mode {
    Mono,
    Chord,
    Fixed,
    FixedChord,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Chord {
    Power,
    Major,
    Minor,
    Sus2,
    Sus4,
    Diminished,
    Augmented,
    Maj7,
    Min7,
    Dom7,
    Dim7,
    HalfDim7,
    Add9,
    Dom9,
    Min9,
}

impl Chord {
    fn to_voicing(self) -> ChordVoicing {
        match self {
            Chord::Power => ChordVoicing::Power,
            Chord::Major => ChordVoicing::Major,
            Chord::Minor => ChordVoicing::Minor,
            Chord::Sus2 => ChordVoicing::Sus2,
            Chord::Sus4 => ChordVoicing::Sus4,
            Chord::Diminished => ChordVoicing::Diminished,
            Chord::Augmented => ChordVoicing::Augmented,
            Chord::Maj7 => ChordVoicing::Maj7,
            Chord::Min7 => ChordVoicing::Min7,
            Chord::Dom7 => ChordVoicing::Dom7,
            Chord::Dim7 => ChordVoicing::Dim7,
            Chord::HalfDim7 => ChordVoicing::HalfDim7,
            Chord::Add9 => ChordVoicing::Add9,
            Chord::Dom9 => ChordVoicing::Dom9,
            Chord::Min9 => ChordVoicing::Min9,
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ScaleKind {
    Chromatic,
    Major,
    Minor,
}

impl SharedCfg {
    pub fn to_config(&self) -> AutoVocoderConfig {
        let scale = match self.scale {
            ScaleKind::Chromatic => Scale::CHROMATIC,
            ScaleKind::Major => Scale::major(self.scale_root % 12),
            ScaleKind::Minor => Scale::minor(self.scale_root % 12),
        };
        let voicing = self.chord.to_voicing();
        let carrier_mode = match self.mode {
            Mode::Mono => CarrierMode::Mono,
            Mode::Chord => CarrierMode::Chord(voicing),
            Mode::Fixed => CarrierMode::Fixed {
                midi: self.fixed_note,
            },
            Mode::FixedChord => CarrierMode::FixedChord {
                midi: self.fixed_note,
                voicing,
            },
        };
        AutoVocoderConfig {
            scale,
            carrier_mode,
            dry_wet: self.mix.clamp(0.0, 1.0),
            portamento_ms: self.portamento.clamp(0.5, 1000.0),
            carrier_level: self.carrier_level.clamp(0.0, 2.0),
            input_gain_db: self.input_gain.clamp(-20.0, 60.0),
            output_gain_db: self.output_gain.clamp(-20.0, 60.0),
            compressor_enabled: !self.no_compress,
            compressor_threshold_db: self.comp_threshold.clamp(-60.0, 0.0),
            ..AutoVocoderConfig::default()
        }
    }
}

fn main() -> Result<()> {
    let args = Cli::parse();
    match args.cmd {
        Cmd::Render(a) => render::run(a),
        #[cfg(feature = "live")]
        Cmd::Live(a) => live::run(a),
    }
}
