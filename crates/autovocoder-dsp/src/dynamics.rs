//! Feedforward soft-knee compressor + simple output gain.
//!
//! Placed at the very end of the autovocoder chain so we can hit a useful
//! output level without the user bolting on a second plugin. Attack/release
//! and ratio are baked to values that suit vocoder output (transient-rich
//! but usually sustained); users tweak threshold (where compression kicks
//! in) and output gain (final makeup).

use crate::filter::EnvFollower;

/// Small, opinionated compressor.
pub struct Compressor {
    env: EnvFollower,
    threshold_db: f32,
    ratio: f32, // compression ratio (e.g. 4.0 => 4:1)
    enabled: bool,
}

impl Compressor {
    pub fn new(sample_rate: f32, threshold_db: f32) -> Self {
        Self {
            // Attack/release tuned for vocoder output: let fast transients
            // through a hair, catch the body of sustained notes.
            env: EnvFollower::new(sample_rate, 5.0, 80.0),
            threshold_db,
            ratio: 4.0,
            enabled: true,
        }
    }

    pub fn set_threshold_db(&mut self, db: f32) {
        self.threshold_db = db.clamp(-60.0, 0.0);
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn reset(&mut self) {
        self.env.reset();
    }

    /// One-sample feedforward compression. Returns the gain-reduced sample
    /// (no makeup — caller applies output_gain afterwards).
    pub fn process(&mut self, x: f32) -> f32 {
        if !self.enabled {
            return x;
        }
        let env = self.env.process(x);
        // Below -120 dB → effectively silence; skip the log.
        if env < 1e-6 {
            return x;
        }
        let env_db = 20.0 * env.log10();
        let over = env_db - self.threshold_db;
        if over <= 0.0 {
            return x;
        }
        // Linear-knee (hard-ish) compression. `(1 - 1/ratio)` is the amount
        // of each dB over threshold that gets squashed.
        let reduction_db = -over * (1.0 - 1.0 / self.ratio);
        let gain = 10f32.powf(reduction_db / 20.0);
        x * gain
    }
}

/// Convert dB to a linear multiplier.
pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_threshold_passes_through() {
        let mut c = Compressor::new(48_000.0, -12.0);
        // Signal at -30 dBFS is below -12 dB threshold; expect no reduction
        // once envelope has settled.
        let x = db_to_linear(-30.0);
        let mut out = 0.0;
        for _ in 0..48_000 {
            out = c.process(x);
        }
        assert!(
            (out - x).abs() < 1e-4,
            "below threshold got attenuated: {out} vs {x}"
        );
    }

    #[test]
    fn above_threshold_gets_reduced() {
        let mut c = Compressor::new(48_000.0, -20.0);
        // Feed a signal at 0 dBFS (well above threshold). Steady state should
        // be quite a bit quieter than the input.
        let x = 1.0;
        let mut out = 0.0;
        for _ in 0..48_000 {
            out = c.process(x);
        }
        assert!(out < 0.5, "expected noticeable reduction, got {out}");
        // With 4:1 ratio and 20 dB over threshold, reduction is 15 dB,
        // i.e. output ~= 10^(-15/20) ≈ 0.178. Allow wide slack.
        assert!(out > 0.05, "reduction too aggressive: {out}");
    }

    #[test]
    fn disabled_is_identity() {
        let mut c = Compressor::new(48_000.0, -40.0);
        c.set_enabled(false);
        for x in [-0.9, 0.0, 0.3, 0.99] {
            assert!((c.process(x) - x).abs() < 1e-9);
        }
    }

    #[test]
    fn db_linear_roundtrip() {
        for db in [-60.0, -20.0, -6.0, 0.0, 6.0, 20.0] {
            let lin = db_to_linear(db);
            let back = 20.0 * lin.log10();
            assert!((back - db).abs() < 1e-4);
        }
    }
}
