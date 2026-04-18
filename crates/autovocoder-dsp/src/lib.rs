//! Core DSP for the autovocoder. Host-agnostic, no I/O.
//!
//! Architecture:
//!   voice → YIN pitch detect → quantize to scale → portamento → saw carrier(s)
//!                                                                     ↓
//!   voice → analysis filterbank → envelope followers ──→ VCA × carrier bands → sum → out

pub mod autovocoder;
pub mod filter;
pub mod osc;
pub mod pitch;
pub mod scale;
pub mod vocoder;

pub use autovocoder::{AutoVocoder, AutoVocoderConfig, CarrierMode};
pub use scale::Scale;
pub use vocoder::VocoderConfig;
