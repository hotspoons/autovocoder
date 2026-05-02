//! Routable modulation LFO (originally just amplitude tremolo).
//!
//! Single oscillator with a sine ↔ square waveshape morph, but the value
//! it produces can be routed to one of several targets in the autovocoder:
//!
//! - **Amplitude** — classic tremolo. Modulates the post-output level.
//! - **Pitch** — vibrato. Modulates the carrier root frequency
//!   multiplicatively.
//! - **DryWet** — sweeps between voice and vocoded mix.
//! - **CarrierLevel** — modulates the synth saw level fed to the vocoder.
//!
//! Shape morph: `shaped = tanh(drive · sin φ) / tanh(drive)` with
//! `drive = 1 + shape² · 24`. shape=0 → essentially sine; shape=1 →
//! near-square (smooth, click-free). See the original tremolo notes for
//! the math.
//!
//! The LFO ticks once per sample regardless of target; the consumer
//! (AutoVocoder) decides where to apply the value. Per-sample helpers
//! convert the shaped LFO into multiplicative or additive factors for
//! each target — caller pre-computes the shaped value once via
//! `tick_lfo()` and feeds it into whichever `apply_*` it needs.

use core::f32::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfoTarget {
    Amplitude,
    Pitch,
    DryWet,
    CarrierLevel,
}

impl LfoTarget {
    /// LV2 port value → target. Out-of-range falls back to Amplitude
    /// (the original tremolo behavior — preserves preset compatibility).
    pub fn from_int(i: i32) -> Self {
        match i {
            1 => LfoTarget::Pitch,
            2 => LfoTarget::DryWet,
            3 => LfoTarget::CarrierLevel,
            _ => LfoTarget::Amplitude,
        }
    }
}

pub struct Tremolo {
    enabled: bool,
    sample_rate: f32,
    lfo_phase: f32,
    lfo_inc: f32,
    depth: f32,
    /// Cached `1 + shape² · 24` — recomputed in `set_shape`.
    drive: f32,
    /// Cached `1 / tanh(drive)`.
    inv_tanh_drive: f32,
    target: LfoTarget,
}

impl Tremolo {
    pub fn new(sample_rate: f32) -> Self {
        let mut t = Self {
            enabled: false,
            sample_rate,
            lfo_phase: 0.0,
            lfo_inc: 5.0 / sample_rate,
            depth: 0.7,
            drive: 1.0,
            inv_tanh_drive: 1.0 / 1.0_f32.tanh(),
            target: LfoTarget::Amplitude,
        };
        t.set_shape(0.0);
        t
    }

    pub fn set_enabled(&mut self, on: bool) {
        if on && !self.enabled {
            // Start near the LFO peak so engaging it doesn't punch a hole
            // (for amplitude target) or yank pitch sharply downward.
            self.lfo_phase = 0.25;
        }
        self.enabled = on;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_rate_hz(&mut self, hz: f32) {
        self.lfo_inc = hz.clamp(0.05, 30.0) / self.sample_rate;
    }

    pub fn set_depth(&mut self, depth_0_1: f32) {
        self.depth = depth_0_1.clamp(0.0, 1.0);
    }

    /// 0.0 = pure sine, 1.0 = near-square. Quadratic taper means most of
    /// the audible morph happens in the upper half of the knob.
    pub fn set_shape(&mut self, shape_0_1: f32) {
        let s = shape_0_1.clamp(0.0, 1.0);
        self.drive = 1.0 + s * s * 24.0;
        self.inv_tanh_drive = 1.0 / self.drive.tanh();
    }

    pub fn set_target(&mut self, t: LfoTarget) {
        self.target = t;
    }

    pub fn target(&self) -> LfoTarget {
        self.target
    }

    pub fn depth(&self) -> f32 {
        self.depth
    }

    pub fn reset(&mut self) {
        self.lfo_phase = 0.0;
    }

    /// Tick the LFO once and return the shaped value in [-1, 1]. When
    /// disabled, returns 0.0 and does *not* advance phase — re-enabling
    /// then picks up cleanly from where it stopped.
    #[inline]
    pub fn tick_lfo(&mut self) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        let lfo = (TAU * self.lfo_phase).sin();
        let shaped = (self.drive * lfo).tanh() * self.inv_tanh_drive;
        self.lfo_phase += self.lfo_inc;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
        }
        shaped
    }

    /// Amplitude target — asymmetric: peak stays at unity, trough goes
    /// to `1 - depth`. Caller multiplies the signal by the result.
    #[inline]
    pub fn amp_gain(&self, shaped: f32) -> f32 {
        1.0 - self.depth * 0.5 * (1.0 - shaped)
    }

    /// Pitch target — multiplicative factor on the carrier root Hz.
    /// Linear approximation around 1.0; max swing at depth=1 is ~±1
    /// semitone (≈ ±6%), which is musical vibrato range.
    #[inline]
    pub fn pitch_mult(&self, shaped: f32) -> f32 {
        1.0 + shaped * self.depth * 0.06
    }

    /// DryWet target — additive offset to add to the base mix. Caller
    /// is responsible for clamping to [0, 1] after adding.
    #[inline]
    pub fn drywet_offset(&self, shaped: f32) -> f32 {
        shaped * self.depth * 0.5
    }

    /// CarrierLevel target — same shape as amp gain but applied to the
    /// carrier sum before vocoding.
    #[inline]
    pub fn carrier_level_mult(&self, shaped: f32) -> f32 {
        1.0 - self.depth * 0.5 * (1.0 - shaped)
    }

    /// Convenience for callers that only want the amplitude target and
    /// don't need to mix the LFO into other paths. Does its own tick.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        if !self.enabled || self.target != LfoTarget::Amplitude {
            return x;
        }
        let shaped = self.tick_lfo();
        x * self.amp_gain(shaped)
    }

    /// Block convenience for the amplitude target. No-op when target is
    /// anything else — the application is the caller's responsibility.
    pub fn process_block(&mut self, buf: &mut [f32]) {
        if !self.enabled || self.target != LfoTarget::Amplitude {
            return;
        }
        for s in buf.iter_mut() {
            let shaped = self.tick_lfo();
            *s *= self.amp_gain(shaped);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_is_identity() {
        let mut t = Tremolo::new(48_000.0);
        for x in [-0.9, 0.0, 0.3, 0.99] {
            assert!((t.process_sample(x) - x).abs() < 1e-9);
        }
    }

    #[test]
    fn full_depth_sine_reaches_zero_and_unity() {
        let mut t = Tremolo::new(48_000.0);
        t.set_enabled(true);
        t.set_rate_hz(10.0);
        t.set_depth(1.0);
        t.set_shape(0.0);
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for _ in 0..(48_000 / 5) {
            let y = t.process_sample(1.0);
            min = min.min(y);
            max = max.max(y);
        }
        assert!(min < 0.02, "trough should reach near-zero, got {min}");
        assert!(max > 0.98, "peak should reach near-one, got {max}");
    }

    #[test]
    fn square_shape_holds_at_extremes() {
        let mut t = Tremolo::new(48_000.0);
        t.set_enabled(true);
        t.set_rate_hz(2.0);
        t.set_depth(1.0);
        t.set_shape(1.0);
        let mut middle = 0;
        let mut total = 0;
        for _ in 0..(48_000 / 2) {
            let y = t.process_sample(1.0);
            if y > 0.3 && y < 0.7 {
                middle += 1;
            }
            total += 1;
        }
        let middle_frac = middle as f32 / total as f32;
        assert!(
            middle_frac < 0.05,
            "near-square should spend <5% of time mid-swing, got {:.1}%",
            middle_frac * 100.0
        );
    }

    #[test]
    fn block_matches_per_sample() {
        let mut a = Tremolo::new(48_000.0);
        let mut b = Tremolo::new(48_000.0);
        for t in [&mut a, &mut b] {
            t.set_enabled(true);
            t.set_rate_hz(7.0);
            t.set_depth(0.6);
            t.set_shape(0.5);
        }
        let n = 1024;
        let input: Vec<f32> = (0..n).map(|i| 0.5 * (i as f32 * 0.02).sin()).collect();
        let out_a: Vec<f32> = input.iter().map(|&x| a.process_sample(x)).collect();
        let mut out_b = input.clone();
        b.process_block(&mut out_b);
        for (i, (p, q)) in out_a.iter().zip(out_b.iter()).enumerate() {
            assert!((p - q).abs() < 1e-6, "drift at {i}: per-sample={p}, block={q}");
        }
    }

    #[test]
    fn non_amplitude_target_passes_through() {
        // process_sample/process_block must be no-ops when target != Amplitude;
        // the AutoVocoder applies the LFO at a different point in the chain.
        let mut t = Tremolo::new(48_000.0);
        t.set_enabled(true);
        t.set_target(LfoTarget::Pitch);
        for x in [-0.7, 0.2, 0.8] {
            assert!((t.process_sample(x) - x).abs() < 1e-9);
        }
    }

    #[test]
    fn tick_lfo_when_disabled_is_zero_and_does_not_advance() {
        let mut t = Tremolo::new(48_000.0);
        // Disabled by default.
        let phase_before = t.lfo_phase;
        for _ in 0..100 {
            assert_eq!(t.tick_lfo(), 0.0);
        }
        assert_eq!(t.lfo_phase, phase_before);
    }
}
