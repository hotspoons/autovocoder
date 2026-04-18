//! WebAssembly bindings for the autovocoder.
//!
//! Designed to be driven from an AudioWorkletProcessor: create one
//! `AutoVocoderWasm` per processor, then call `process` on each 128-sample
//! block with an in-place Float32Array. DSP state lives in Rust memory —
//! the JS side just passes block pointers in via wasm-bindgen's typed-array
//! marshalling.

use autovocoder_dsp::{AutoVocoder, AutoVocoderConfig, CarrierMode, Scale};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct AutoVocoderWasm {
    inner: AutoVocoder,
}

#[wasm_bindgen]
impl AutoVocoderWasm {
    #[wasm_bindgen(constructor)]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: AutoVocoder::new(sample_rate, AutoVocoderConfig::default()),
        }
    }

    /// Use a fixed-pitch carrier locked to a MIDI note (Soundwave style).
    pub fn set_fixed_note(&mut self, midi: u8) {
        self.inner.set_carrier_mode(CarrierMode::Fixed { midi });
    }

    /// Mono carrier that tracks the quantized input pitch.
    pub fn set_mono(&mut self) {
        self.inner.set_carrier_mode(CarrierMode::Mono);
    }

    /// Carrier spreads to a major triad on each detected note.
    pub fn set_major_triad(&mut self) {
        self.inner.set_carrier_mode(CarrierMode::major_triad());
    }

    /// Carrier spreads to a minor triad on each detected note.
    pub fn set_minor_triad(&mut self) {
        self.inner.set_carrier_mode(CarrierMode::minor_triad());
    }

    pub fn set_chromatic(&mut self) {
        self.inner.set_scale(Scale::CHROMATIC);
    }

    pub fn set_major_scale(&mut self, root_pc: u8) {
        self.inner.set_scale(Scale::major(root_pc % 12));
    }

    pub fn set_minor_scale(&mut self, root_pc: u8) {
        self.inner.set_scale(Scale::minor(root_pc % 12));
    }

    pub fn set_dry_wet(&mut self, mix: f32) {
        self.inner.set_dry_wet(mix);
    }

    pub fn reset(&mut self) {
        self.inner.reset();
    }

    /// Process one mono block in place. Pass the same Float32Array each call.
    pub fn process(&mut self, block: &mut [f32]) {
        self.inner.process_buffer(block);
    }
}
