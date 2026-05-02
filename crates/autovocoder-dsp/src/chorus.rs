//! Two-voice mono chorus.
//!
//! Single delay line read at two LFO-modulated taps phase-offset by 180°.
//! When voice A is at peak delay, voice B is at minimum delay — the classic
//! "two detuned voices" character that's both wider and richer than a
//! single-voice vibrato. Linear interpolation on the read taps for cheap
//! fractional-sample delay.
//!
//! State is per-instance — when the same chorus settings drive two
//! placements (carrier and output), the LFOs drift apart over time, which
//! gives the two locations un-correlated motion. That's musically more
//! useful than a strictly-correlated single LFO.

use core::f32::consts::TAU;

/// Maximum total delay (base + LFO peak) in milliseconds. Caps the buffer
/// allocation; user-facing depth is clamped to keep `base + depth ≤ MAX`.
const MAX_DELAY_MS: f32 = 30.0;
/// Center of the LFO swing. Chosen so even at zero depth the signal has a
/// little Haas-style thickening rather than collapsing onto the dry tap.
const BASE_DELAY_MS: f32 = 8.0;

pub struct Chorus {
    enabled: bool,
    sample_rate: f32,
    /// Power-of-two delay line so the read indices use `& mask` instead of `%`.
    buf: Vec<f32>,
    mask: usize,
    write_idx: usize,
    /// Shared LFO phase in [0, 1). Voice B reads at phase + 0.5 (180° off).
    lfo_phase: f32,
    lfo_inc: f32,
    base_delay_samples: f32,
    /// Peak delay deviation in samples. Capped so `base ± depth` stays in
    /// `[1, MAX_DELAY_MS]` — keeps the read pointer away from the write
    /// pointer (avoids comb-filter buzz when crossing).
    depth_samples: f32,
    mix: f32,
}

impl Chorus {
    pub fn new(sample_rate: f32) -> Self {
        let max_samples = (MAX_DELAY_MS * 1e-3 * sample_rate).ceil() as usize + 2;
        let buf_len = max_samples.next_power_of_two().max(2);
        Self {
            enabled: false,
            sample_rate,
            buf: vec![0.0; buf_len],
            mask: buf_len - 1,
            write_idx: 0,
            lfo_phase: 0.0,
            lfo_inc: 0.7 / sample_rate,
            base_delay_samples: BASE_DELAY_MS * 1e-3 * sample_rate,
            depth_samples: 0.5 * 5e-3 * sample_rate, // 0.5 normalized → ~2.5ms peak
            mix: 0.5,
        }
    }

    pub fn set_enabled(&mut self, on: bool) {
        // Reset on enable so a stale buffer of nonsense doesn't get heard
        // the first time it's re-engaged.
        if on && !self.enabled {
            self.reset();
        }
        self.enabled = on;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_rate_hz(&mut self, hz: f32) {
        self.lfo_inc = hz.clamp(0.05, 10.0) / self.sample_rate;
    }

    /// Depth as a normalized 0..1 control. Mapped to a peak deviation of
    /// 0..5 ms — enough swing for a strong chorus without skirting the
    /// write pointer.
    pub fn set_depth(&mut self, depth_0_1: f32) {
        let depth_ms = depth_0_1.clamp(0.0, 1.0) * 5.0;
        let max_depth_ms = (MAX_DELAY_MS - BASE_DELAY_MS - 1.0).max(0.0);
        let clamped_ms = depth_ms.min(max_depth_ms);
        self.depth_samples = clamped_ms * 1e-3 * self.sample_rate;
    }

    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    pub fn reset(&mut self) {
        for s in &mut self.buf {
            *s = 0.0;
        }
        self.write_idx = 0;
        self.lfo_phase = 0.0;
    }

    /// Process one sample. Branches once on `enabled`; in the disabled
    /// case returns the input untouched.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        if self.enabled {
            self.process_one(x)
        } else {
            x
        }
    }

    /// Block variant. Skips the loop entirely when disabled — saves the
    /// per-sample branch and the LFO advance.
    pub fn process_block(&mut self, buf: &mut [f32]) {
        if !self.enabled {
            return;
        }
        for s in buf.iter_mut() {
            *s = self.process_one(*s);
        }
    }

    #[inline(always)]
    fn process_one(&mut self, x: f32) -> f32 {
        // Write the new sample first so the read positions work against an
        // up-to-date buffer (matters at very short delays).
        self.buf[self.write_idx] = x;

        // LFO — single sin call shared between the two voices. Voice B
        // takes the negation, which is sin(phase + π).
        let lfo_a = (TAU * self.lfo_phase).sin();
        let lfo_b = -lfo_a;

        let voice_a = self.read_tap(self.base_delay_samples + self.depth_samples * lfo_a);
        let voice_b = self.read_tap(self.base_delay_samples + self.depth_samples * lfo_b);
        // Two voices summed and halved: keeps level consistent vs single-voice.
        let wet = 0.5 * (voice_a + voice_b);

        // Advance state.
        self.write_idx = (self.write_idx + 1) & self.mask;
        self.lfo_phase += self.lfo_inc;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
        }

        // Equal-power-ish mix. Linear is fine for a wet that's already a
        // delayed copy of the dry — phase relationship matters more than
        // exact gain matching here.
        x * (1.0 - self.mix) + wet * self.mix
    }

    /// Read a fractional-delay tap, linearly interpolated.
    #[inline(always)]
    fn read_tap(&self, delay_samples: f32) -> f32 {
        let d = delay_samples.max(1.0);
        let d_int = d as usize;
        let frac = d - d_int as f32;
        // write_idx already points at the *next* slot, so the most recent
        // sample is at write_idx - 1. Step back further by `d_int`.
        let base = (self.write_idx + self.buf.len() - d_int) & self.mask;
        let prev = (base + self.buf.len() - 1) & self.mask;
        let s0 = self.buf[base];
        let s1 = self.buf[prev];
        s0 + (s1 - s0) * frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_is_identity() {
        let mut c = Chorus::new(48_000.0);
        // Default: not enabled.
        for x in [-0.7, 0.0, 0.3, 0.99] {
            assert!((c.process_sample(x) - x).abs() < 1e-9);
        }
    }

    #[test]
    fn enabled_modulates_dc_into_lfo() {
        // Driving a DC input through the chorus should produce a DC output
        // (delayed copies of a constant are still that constant). Exercises
        // the read-tap math without relying on phase.
        let mut c = Chorus::new(48_000.0);
        c.set_enabled(true);
        c.set_rate_hz(2.0);
        c.set_depth(1.0);
        c.set_mix(1.0);
        // Run for a while to fill the delay line.
        let mut last = 0.0;
        for _ in 0..(48_000 / 4) {
            last = c.process_sample(0.5);
        }
        assert!((last - 0.5).abs() < 0.05, "DC stays DC, got {last}");
    }

    #[test]
    fn enabled_block_matches_per_sample() {
        let mut a = Chorus::new(48_000.0);
        let mut b = Chorus::new(48_000.0);
        for c in [&mut a, &mut b] {
            c.set_enabled(true);
            c.set_rate_hz(1.5);
            c.set_depth(0.4);
            c.set_mix(0.5);
        }
        let n = 2048;
        let input: Vec<f32> = (0..n)
            .map(|i| 0.3 * (i as f32 * 0.01).sin())
            .collect();
        let out_per_sample: Vec<f32> = input.iter().map(|&x| a.process_sample(x)).collect();
        let mut out_block = input.clone();
        b.process_block(&mut out_block);
        for (i, (p, q)) in out_per_sample.iter().zip(out_block.iter()).enumerate() {
            assert!((p - q).abs() < 1e-6, "drift at {i}: per-sample={p}, block={q}");
        }
    }
}
