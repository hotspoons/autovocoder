//! Standalone autovocoder: two subcommands.
//!
//!   autovocoder render --input voice.wav --output out.wav [--mode ...]
//!   autovocoder live   [--mode ...]     (requires `live` feature)
//!
//! The `render` path is the offline WAV-in / WAV-out pipeline used for
//! iterating on DSP. The `live` path runs mic → autovocoder → speakers
//! through cpal, for auditioning on a laptop without involving a DAW.

use anyhow::Result;
use autovocoder_dsp::{AutoVocoderConfig, CarrierMode, Scale};
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
    #[arg(long, value_enum, default_value_t = Mode::Mono)]
    pub mode: Mode,

    /// Fixed MIDI note for `fixed` mode (default: 48 = C3, Soundwave-ish).
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
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Mode {
    Mono,
    MajorTriad,
    MinorTriad,
    Fixed,
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
        let carrier_mode = match self.mode {
            Mode::Mono => CarrierMode::Mono,
            Mode::MajorTriad => CarrierMode::major_triad(),
            Mode::MinorTriad => CarrierMode::minor_triad(),
            Mode::Fixed => CarrierMode::Fixed {
                midi: self.fixed_note,
            },
        };
        AutoVocoderConfig {
            scale,
            carrier_mode,
            dry_wet: self.mix.clamp(0.0, 1.0),
            portamento_ms: self.portamento.clamp(0.5, 1000.0),
            carrier_level: self.carrier_level.clamp(0.0, 2.0),
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
