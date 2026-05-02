//! Bit-depth + sample-rate reduction. Instant lo-fi / 8-bit / NES robot.
//!
//! Two orthogonal degradations:
//!   - **Bit depth** — quantize the sample to a power-of-two ladder of
//!     levels. Lower bit count → coarser steps → audible distortion that
//!     correlates with the signal (it's not noise, it's quantization).
//!   - **Sample rate** — sample-and-hold for N input samples between
//!     updates. Upper-frequency content gets aliased down (the ugly part
//!     of digital aliasing — but here that's the point).
//!
//! Both controls let you dial back to "off": `bits = 16` and `rate = 1.0`
//! is a near-bypass (still quantizes to a 16-bit ladder, which is below
//! audibility for typical material).

pub struct BitCrusher {
    enabled: bool,
    /// Effective bit depth, clamped to 1..16.
    bits: u32,
    /// Samples between sample-and-hold updates. 1 = no reduction.
    hold_samples: u32,
    held: f32,
    counter: u32,
}

impl BitCrusher {
    pub fn new() -> Self {
        Self {
            enabled: false,
            bits: 16,
            hold_samples: 1,
            held: 0.0,
            counter: 0,
        }
    }

    pub fn set_enabled(&mut self, on: bool) {
        if on && !self.enabled {
            self.reset();
        }
        self.enabled = on;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Set effective bit depth (1..16). Lower = more crush.
    pub fn set_bits(&mut self, bits: f32) {
        self.bits = bits.clamp(1.0, 16.0).round() as u32;
    }

    /// Set sample-rate-reduction amount as a 0..1 normalized control.
    /// 1.0 = full sample rate (no reduction); 0.0 = max reduction (hold for
    /// 32 input samples, e.g. ~1.5 kHz at 48k).
    pub fn set_rate(&mut self, rate_0_1: f32) {
        let r = rate_0_1.clamp(0.0, 1.0);
        // Linear map: rate=1 → 1 sample, rate=0 → 32 samples held.
        self.hold_samples = (1.0 + (1.0 - r) * 31.0).round() as u32;
    }

    pub fn reset(&mut self) {
        self.held = 0.0;
        self.counter = 0;
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        if self.enabled {
            self.process_one(x)
        } else {
            x
        }
    }

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
        // Sample-and-hold first — only re-quantize on update boundaries
        // so the dropped samples actually get dropped, not just resampled.
        if self.counter == 0 {
            // Quantize to `bits` levels around zero. Two's-complement-style
            // ladder: levels = 2^(bits-1), step = 1 / levels.
            let levels = (1u32 << self.bits.min(15).saturating_sub(1)) as f32;
            self.held = (x * levels).round() / levels;
        }
        self.counter += 1;
        if self.counter >= self.hold_samples {
            self.counter = 0;
        }
        self.held
    }
}

impl Default for BitCrusher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_is_identity() {
        let mut c = BitCrusher::new();
        for x in [-0.9, 0.0, 0.3, 0.99] {
            assert!((c.process_sample(x) - x).abs() < 1e-9);
        }
    }

    #[test]
    fn low_bits_quantizes_to_few_levels() {
        let mut c = BitCrusher::new();
        c.set_enabled(true);
        c.set_bits(2.0); // levels = 2 → 3 quantized values: -1, 0, +0.5? actually -1, 0, 1
        c.set_rate(1.0);
        // Sweep input, collect distinct outputs.
        let mut outs: Vec<i32> = (0..200)
            .map(|i| (c.process_sample(i as f32 / 200.0 - 0.5) * 100.0).round() as i32)
            .collect();
        outs.sort_unstable();
        outs.dedup();
        // 2 bits → at most ~5 distinct levels. Assert << 200.
        assert!(outs.len() <= 8, "expected few quantization levels, got {}", outs.len());
    }

    #[test]
    fn rate_reduction_holds_value() {
        let mut c = BitCrusher::new();
        c.set_enabled(true);
        c.set_bits(16.0);
        c.set_rate(0.0); // hold for 32 samples
        let first = c.process_sample(0.5);
        // Next 31 samples should output the same held value regardless of input.
        for i in 1..32 {
            let y = c.process_sample(-0.7); // input differs every call
            assert!(
                (y - first).abs() < 1e-6,
                "sample {i}: {y} differs from held {first}"
            );
        }
        // 33rd sample should pick up new input.
        let next = c.process_sample(-0.7);
        assert!(
            (next - first).abs() > 0.1,
            "expected fresh sample after hold period"
        );
    }
}
