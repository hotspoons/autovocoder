//! Core DSP for the autovocoder. Host-agnostic, no I/O.
//!
//! Architecture:
//!   voice → YIN pitch detect → quantize to scale → portamento → saw carrier(s)
//!                                                                     ↓
//!   voice → analysis filterbank → envelope followers ──→ VCA × carrier bands → sum → out

pub mod autovocoder;
pub mod chorus;
pub mod crusher;
pub mod dynamics;
pub mod filter;
pub mod osc;
pub mod pitch;
pub mod saturate;
pub mod scale;
pub mod tremolo;
pub mod vocoder;

pub use autovocoder::{AutoVocoder, AutoVocoderConfig, CarrierMode, ChordVoicing};
pub use chorus::Chorus;
pub use crusher::BitCrusher;
pub use pitch::{PitchAlgorithm, PitchDetector, PitchEstimate};
pub use saturate::{DriveMode, Saturator};
pub use scale::Scale;
pub use tremolo::{LfoTarget, Tremolo};
pub use vocoder::VocoderConfig;
