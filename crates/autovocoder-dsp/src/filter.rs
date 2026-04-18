//! Biquad filters (RBJ cookbook) and envelope follower.

use core::f32::consts::TAU;

/// Transposed-Direct-Form-II biquad. Cheap, stable, good for audio.
#[derive(Clone, Copy, Default)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// RBJ bandpass (constant 0 dB peak gain).
    pub fn bandpass(sample_rate: f32, center_hz: f32, q: f32) -> Self {
        let w0 = TAU * center_hz / sample_rate;
        let (sin_w0, cos_w0) = (w0.sin(), w0.cos());
        let alpha = sin_w0 / (2.0 * q.max(1e-4));

        let a0 = 1.0 + alpha;
        let b0 = alpha / a0;
        let b1 = 0.0;
        let b2 = -alpha / a0;
        let a1 = -2.0 * cos_w0 / a0;
        let a2 = (1.0 - alpha) / a0;
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Cascade two identical biquads for a steeper (4-pole) bandpass.
#[derive(Clone, Copy, Default)]
pub struct BandPass4 {
    a: Biquad,
    b: Biquad,
}

impl BandPass4 {
    pub fn new(sample_rate: f32, center_hz: f32, q: f32) -> Self {
        let bq = Biquad::bandpass(sample_rate, center_hz, q);
        Self { a: bq, b: bq }
    }

    pub fn process(&mut self, x: f32) -> f32 {
        self.b.process(self.a.process(x))
    }

    pub fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
    }
}

/// One-pole envelope follower with separate attack/release time constants.
/// Tracks |x| with an asymmetric smoother — standard vocoder envelope detector.
#[derive(Clone, Copy)]
pub struct EnvFollower {
    attack_coeff: f32,
    release_coeff: f32,
    env: f32,
}

impl EnvFollower {
    pub fn new(sample_rate: f32, attack_ms: f32, release_ms: f32) -> Self {
        Self {
            attack_coeff: time_to_coeff(sample_rate, attack_ms),
            release_coeff: time_to_coeff(sample_rate, release_ms),
            env: 0.0,
        }
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let rect = x.abs();
        let coeff = if rect > self.env {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.env += coeff * (rect - self.env);
        self.env
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
    }
}

/// Convert a time constant in ms to a one-pole smoothing coefficient.
/// tau = -1 / (sr * ln(1 - a))  →  a = 1 - exp(-1 / (sr * t_sec))
fn time_to_coeff(sample_rate: f32, ms: f32) -> f32 {
    if ms <= 0.0 {
        return 1.0;
    }
    let t_sec = ms * 1e-3;
    1.0 - (-1.0 / (sample_rate * t_sec)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bandpass_passes_center_attenuates_far() {
        // Feed a sine at center, compare RMS vs far-off sine.
        let sr = 48_000.0;
        let fc = 1000.0;
        let mut bp = BandPass4::new(sr, fc, 2.0);

        let rms = |freq: f32| {
            let mut bp = BandPass4::new(sr, fc, 2.0);
            let mut acc = 0.0f32;
            let n = 8192;
            // warmup then measure
            for i in 0..(n * 2) {
                let t = i as f32 / sr;
                let x = (TAU * freq * t).sin();
                let y = bp.process(x);
                if i >= n {
                    acc += y * y;
                }
            }
            (acc / n as f32).sqrt()
        };
        let _ = bp.process(0.0); // silence unused warning
        let passband = rms(fc);
        let stopband = rms(fc * 10.0); // decade away
        assert!(
            passband > stopband * 5.0,
            "expected strong attenuation 1 decade away: pb={passband}, sb={stopband}"
        );
    }

    #[test]
    fn env_follower_attack_then_release() {
        let sr = 48_000.0;
        let mut env = EnvFollower::new(sr, 1.0, 20.0);
        // Constant 1.0 for a while — should rise toward 1.
        let mut out = 0.0;
        for _ in 0..(sr as usize / 10) {
            out = env.process(1.0);
        }
        assert!(out > 0.9, "env should attack to ~1: {out}");

        // Now zero input — should release toward 0.
        for _ in 0..(sr as usize / 2) {
            out = env.process(0.0);
        }
        assert!(out < 0.05, "env should release to ~0: {out}");
    }
}
