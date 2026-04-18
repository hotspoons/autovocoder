//! Top-level autovocoder: pitch-detect the voice, quantize to scale,
//! synthesize a carrier, drive the vocoder.

use crate::dynamics::{db_to_linear, Compressor};
use crate::osc::Saw;
use crate::pitch::{PitchEstimate, YinDetector};
use crate::scale::{midi_to_hz, quantize_hz_to_scale, Portamento, Scale};
use crate::vocoder::{Vocoder, VocoderConfig};

/// Carrier voicing mode.
#[derive(Clone, Copy, Debug)]
pub enum CarrierMode {
    /// One saw at the quantized pitch.
    Mono,
    /// Root + major third + fifth (in semitones from the root).
    Chord { third_semis: i8, fifth_semis: i8 },
    /// Force a fixed MIDI note regardless of input pitch — classic Soundwave.
    Fixed { midi: u8 },
}

impl CarrierMode {
    pub fn major_triad() -> Self {
        Self::Chord {
            third_semis: 4,
            fifth_semis: 7,
        }
    }
    pub fn minor_triad() -> Self {
        Self::Chord {
            third_semis: 3,
            fifth_semis: 7,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AutoVocoderConfig {
    pub vocoder: VocoderConfig,
    pub scale: Scale,
    pub carrier_mode: CarrierMode,
    pub portamento_ms: f32,
    pub dry_wet: f32,       // 0.0 = only voice, 1.0 = only vocoded
    pub carrier_level: f32, // saw level fed to the vocoder
    pub pitch_min_hz: f32,
    pub pitch_max_hz: f32,
    // Output stage.
    pub output_gain_db: f32, // applied after compressor
    pub compressor_enabled: bool,
    pub compressor_threshold_db: f32,
}

impl Default for AutoVocoderConfig {
    fn default() -> Self {
        Self {
            vocoder: VocoderConfig::default(),
            scale: Scale::CHROMATIC,
            carrier_mode: CarrierMode::Mono,
            portamento_ms: 25.0,
            dry_wet: 1.0,
            carrier_level: 0.6,
            pitch_min_hz: 70.0,
            pitch_max_hz: 800.0,
            // Vocoders are inherently quiet vs input (most energy lives
            // outside any single band). These defaults make the plugin
            // useful straight out of the box without an external makeup.
            output_gain_db: 9.0,
            compressor_enabled: true,
            compressor_threshold_db: -18.0,
        }
    }
}

pub struct AutoVocoder {
    sample_rate: f32,
    cfg: AutoVocoderConfig,
    yin: YinDetector,
    last_pitch: PitchEstimate,
    // Up to 3 carrier oscs (chord mode). Unused ones are kept at 0 Hz.
    oscs: [Saw; 3],
    portos: [Portamento; 3],
    vocoder: Vocoder,
    compressor: Compressor,
    output_gain: f32, // linear, derived from output_gain_db
}

impl AutoVocoder {
    pub fn new(sample_rate: f32, cfg: AutoVocoderConfig) -> Self {
        let yin = YinDetector::new(sample_rate, cfg.pitch_min_hz, cfg.pitch_max_hz, 256);
        let mk_porto = || Portamento::new(sample_rate, cfg.portamento_ms);
        let mut compressor = Compressor::new(sample_rate, cfg.compressor_threshold_db);
        compressor.set_enabled(cfg.compressor_enabled);
        Self {
            sample_rate,
            yin,
            last_pitch: PitchEstimate::UNVOICED,
            oscs: [
                Saw::new(sample_rate),
                Saw::new(sample_rate),
                Saw::new(sample_rate),
            ],
            portos: [mk_porto(), mk_porto(), mk_porto()],
            vocoder: Vocoder::new(sample_rate, cfg.vocoder),
            compressor,
            output_gain: db_to_linear(cfg.output_gain_db),
            cfg,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn set_scale(&mut self, scale: Scale) {
        self.cfg.scale = scale;
    }

    pub fn set_carrier_mode(&mut self, mode: CarrierMode) {
        self.cfg.carrier_mode = mode;
    }

    pub fn set_dry_wet(&mut self, mix: f32) {
        self.cfg.dry_wet = mix.clamp(0.0, 1.0);
    }

    pub fn set_portamento_ms(&mut self, ms: f32) {
        let clamped = ms.clamp(0.5, 1000.0);
        self.cfg.portamento_ms = clamped;
        for p in &mut self.portos {
            p.set_time(self.sample_rate, clamped);
        }
    }

    pub fn set_carrier_level(&mut self, level: f32) {
        self.cfg.carrier_level = level.clamp(0.0, 2.0);
    }

    pub fn set_output_gain_db(&mut self, db: f32) {
        let clamped = db.clamp(-20.0, 60.0);
        self.cfg.output_gain_db = clamped;
        self.output_gain = db_to_linear(clamped);
    }

    pub fn set_compressor_enabled(&mut self, on: bool) {
        self.cfg.compressor_enabled = on;
        self.compressor.set_enabled(on);
    }

    pub fn set_compressor_threshold_db(&mut self, db: f32) {
        self.cfg.compressor_threshold_db = db;
        self.compressor.set_threshold_db(db);
    }

    pub fn reset(&mut self) {
        self.vocoder.reset();
        self.compressor.reset();
        self.last_pitch = PitchEstimate::UNVOICED;
        for o in &mut self.oscs {
            o.reset_phase();
        }
        for p in &mut self.portos {
            p.reset();
        }
    }

    /// Target Hz for each of the 3 carrier oscs given a base voice-pitch.
    /// Unused slots return 0.0 (silenced).
    fn carrier_targets(&self, voice_hz: f32) -> [f32; 3] {
        let base_hz = match self.cfg.carrier_mode {
            CarrierMode::Fixed { midi } => midi_to_hz(midi as f32),
            _ => quantize_hz_to_scale(voice_hz, self.cfg.scale),
        };
        if base_hz <= 0.0 {
            return [0.0; 3];
        }
        match self.cfg.carrier_mode {
            CarrierMode::Mono | CarrierMode::Fixed { .. } => [base_hz, 0.0, 0.0],
            CarrierMode::Chord {
                third_semis,
                fifth_semis,
            } => {
                let ratio = |semis: i8| 2f32.powf(semis as f32 / 12.0);
                [
                    base_hz,
                    base_hz * ratio(third_semis),
                    base_hz * ratio(fifth_semis),
                ]
            }
        }
    }

    /// Process one sample. Voice in → vocoded out.
    pub fn process_sample(&mut self, voice: f32) -> f32 {
        if let Some(est) = self.yin.push(voice) {
            if est.is_voiced() {
                self.last_pitch = est;
            }
        }

        let targets = self.carrier_targets(self.last_pitch.hz);
        let mut carrier_sum = 0.0;
        let mut active = 0;
        for ((osc, porto), target) in self
            .oscs
            .iter_mut()
            .zip(self.portos.iter_mut())
            .zip(targets)
        {
            let smoothed = porto.process(target);
            if smoothed > 0.0 {
                osc.set_frequency(smoothed);
                carrier_sum += osc.tick();
                active += 1;
            }
        }
        if active > 1 {
            // Normalize so chord mode doesn't blow up vs mono.
            carrier_sum /= (active as f32).sqrt();
        }
        carrier_sum *= self.cfg.carrier_level;

        let wet = self.vocoder.process(voice, carrier_sum);
        let mix = self.cfg.dry_wet;
        let mixed = voice * (1.0 - mix) + wet * mix;
        // Output stage: GAIN FIRST, then compressor. The vocoder output is
        // intrinsically very quiet; if we compressed first, the signal
        // would still be below threshold and the compressor would do
        // nothing. Applying gain first means the compressor actually sees
        // a loud signal and catches peaks — letting users crank gain hard
        // without clipping. Final soft-clamp is belt-and-suspenders for
        // extreme gain settings where the compressor can't quite keep up.
        let gained = mixed * self.output_gain;
        let compressed = self.compressor.process(gained);
        compressed.clamp(-0.98, 0.98)
    }

    /// Process a buffer in place for convenience.
    pub fn process_buffer(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.process_sample(*s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::TAU;

    #[test]
    fn silence_in_silence_out() {
        let mut av = AutoVocoder::new(48_000.0, AutoVocoderConfig::default());
        let mut buf = vec![0.0f32; 8192];
        av.process_buffer(&mut buf);
        let peak = buf.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        assert!(peak < 1e-3, "silence should stay silent, peak={peak}");
    }

    #[test]
    fn voiced_input_produces_output() {
        let sr = 48_000.0;
        let cfg = AutoVocoderConfig {
            carrier_mode: CarrierMode::Fixed { midi: 48 }, // C3
            ..AutoVocoderConfig::default()
        };
        let mut av = AutoVocoder::new(sr, cfg);
        // Harmonically-rich fake voice: sum of 6 partials at 200 Hz fundamental.
        let mut buf: Vec<f32> = (0..sr as usize * 2)
            .map(|i| {
                let t = i as f32 / sr;
                let mut x = 0.0;
                for h in 1..=6 {
                    x += (TAU * 200.0 * h as f32 * t).sin() / h as f32;
                }
                0.3 * x
            })
            .collect();
        av.process_buffer(&mut buf);
        // Measure RMS over the latter half (past warmup).
        let tail = &buf[buf.len() / 2..];
        let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 0.01, "expected audible output, rms={rms}");
    }
}
