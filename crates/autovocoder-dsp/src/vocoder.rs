//! Classic analog-style channel vocoder.
//!
//! Per band N:
//!   analysis_bp_N(voice) → envelope_N
//!   synth_bp_N(carrier)  * envelope_N  → summed to output
//!
//! Bands are log-spaced across the speech range. Matched analysis and
//! synthesis filters keep phase/timing aligned between the two paths.

use crate::filter::{BandPass4, EnvFollower};

/// Vocoder configuration.
#[derive(Clone, Copy, Debug)]
pub struct VocoderConfig {
    pub bands: usize,
    pub f_low: f32,
    pub f_high: f32,
    pub q: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
}

impl Default for VocoderConfig {
    fn default() -> Self {
        // Soundwave-ish defaults: fast attack, medium release, 16 bands
        // across the speech formant range. Q=6 gives ~1/6-octave bands.
        Self {
            bands: 16,
            f_low: 110.0,
            f_high: 8_000.0,
            q: 6.0,
            attack_ms: 3.0,
            release_ms: 20.0,
        }
    }
}

struct Band {
    analysis: BandPass4,
    synthesis: BandPass4,
    env: EnvFollower,
}

pub struct Vocoder {
    bands: Vec<Band>,
    output_gain: f32,
}

impl Vocoder {
    pub fn new(sample_rate: f32, cfg: VocoderConfig) -> Self {
        let bands = (0..cfg.bands)
            .map(|i| {
                let f = log_center(i, cfg.bands, cfg.f_low, cfg.f_high);
                Band {
                    analysis: BandPass4::new(sample_rate, f, cfg.q),
                    synthesis: BandPass4::new(sample_rate, f, cfg.q),
                    env: EnvFollower::new(sample_rate, cfg.attack_ms, cfg.release_ms),
                }
            })
            .collect();
        // Empirical gain comp — more bands and higher Q means less overlap.
        let output_gain = 2.0 * (cfg.bands as f32 / 16.0).sqrt();
        Self { bands, output_gain }
    }

    pub fn reset(&mut self) {
        for b in &mut self.bands {
            b.analysis.reset();
            b.synthesis.reset();
            b.env.reset();
        }
    }

    /// Process one sample: `modulator` is the voice, `carrier` the synth.
    pub fn process(&mut self, modulator: f32, carrier: f32) -> f32 {
        let mut out = 0.0;
        for b in &mut self.bands {
            let mod_band = b.analysis.process(modulator);
            let env = b.env.process(mod_band);
            let car_band = b.synthesis.process(carrier);
            out += car_band * env;
        }
        out * self.output_gain
    }
}

/// Log-spaced center frequency for band `i` of `n` in [f_low, f_high].
fn log_center(i: usize, n: usize, f_low: f32, f_high: f32) -> f32 {
    if n <= 1 {
        return (f_low * f_high).sqrt();
    }
    let lo = f_low.ln();
    let hi = f_high.ln();
    let t = (i as f32 + 0.5) / n as f32; // centered within each bucket
    (lo + (hi - lo) * t).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::TAU;

    #[test]
    fn log_spacing_monotonic() {
        let n = 16;
        let mut prev = 0.0;
        for i in 0..n {
            let f = log_center(i, n, 100.0, 10_000.0);
            assert!(f > prev);
            prev = f;
        }
    }

    #[test]
    fn silent_modulator_produces_silence() {
        let sr = 48_000.0;
        let mut v = Vocoder::new(sr, VocoderConfig::default());
        let mut peak = 0.0f32;
        for i in 0..sr as usize {
            // Carrier is a loud sawtooth-ish signal; modulator is silent.
            let c = (TAU * 220.0 * i as f32 / sr).sin();
            let y = v.process(0.0, c).abs();
            if i > 4800 {
                peak = peak.max(y);
            }
        }
        assert!(peak < 0.05, "expected near-silence, got peak {peak}");
    }

    #[test]
    fn voiced_modulator_produces_output() {
        let sr = 48_000.0;
        let mut v = Vocoder::new(sr, VocoderConfig::default());
        let mut rms_acc = 0.0f32;
        let mut n = 0;
        for i in 0..sr as usize {
            // Voice = sustained vowel-ish sine; carrier = different freq saw.
            let m = (TAU * 300.0 * i as f32 / sr).sin();
            let c = (TAU * 150.0 * i as f32 / sr).sin() + (TAU * 450.0 * i as f32 / sr).sin() * 0.5;
            let y = v.process(m, c);
            if i > 9600 {
                rms_acc += y * y;
                n += 1;
            }
        }
        let rms = (rms_acc / n as f32).sqrt();
        assert!(rms > 0.01, "expected audible output, got RMS {rms}");
    }
}
