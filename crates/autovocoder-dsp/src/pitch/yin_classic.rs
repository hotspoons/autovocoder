//! Classic time-domain YIN, rewritten around a linear scratch buffer.
//!
//! The original ring-buffer indexing did `% window` on every access inside
//! the hot inner loop — two modulos per inner iteration over W/2 × max_τ
//! samples per hop. The compiler can't prove `window` is a power of two at
//! that callsite, so it emitted real divides and the loop never autovectorized.
//!
//! Fix: snapshot the ring into a linear scratch slice once per hop, then run
//! the difference function on contiguous memory. The inner loop now reduces
//! to a tight `(a - b) * (a - b)` accumulation over two slices that any
//! vectorizing backend can tile into SIMD lanes.

use super::{parabolic_interp, PitchEstimate};

pub struct YinDetector {
    sample_rate: f32,
    window: usize,
    hop: usize,
    threshold: f32,
    /// Ring buffer holding the most recent `window` input samples.
    buf: Vec<f32>,
    write_idx: usize,
    samples_since_hop: usize,
    /// Linear, hop-aligned snapshot of `buf`. Reused; one memcpy per hop.
    linear: Vec<f32>,
    /// d'(τ) scratch.
    cmnd: Vec<f32>,
    min_tau: usize,
    max_tau: usize,
}

impl YinDetector {
    pub fn new(sample_rate: f32, min_hz: f32, max_hz: f32, hop: usize) -> Self {
        let min_tau = (sample_rate / max_hz).floor() as usize;
        let max_tau = (sample_rate / min_hz).ceil() as usize;
        let window = (2 * max_tau).next_power_of_two();
        Self {
            sample_rate,
            window,
            hop: hop.max(1),
            threshold: 0.15,
            buf: vec![0.0; window],
            write_idx: 0,
            samples_since_hop: 0,
            linear: vec![0.0; window],
            cmnd: vec![0.0; max_tau + 1],
            min_tau: min_tau.max(2),
            max_tau,
        }
    }

    pub fn set_threshold(&mut self, t: f32) {
        self.threshold = t.clamp(0.01, 0.5);
    }

    #[inline]
    pub fn push(&mut self, x: f32) -> Option<PitchEstimate> {
        // Power-of-two `window` lets the compiler pick `& (window-1)` here
        // since `self.window` is loaded as a runtime value but the LLVM
        // peepholes typically detect the pattern.
        self.buf[self.write_idx] = x;
        self.write_idx = (self.write_idx + 1) % self.window;
        self.samples_since_hop += 1;
        if self.samples_since_hop >= self.hop {
            self.samples_since_hop = 0;
            Some(self.estimate())
        } else {
            None
        }
    }

    fn estimate(&mut self) -> PitchEstimate {
        // Snapshot the ring buffer to linear scratch — one O(W) memcpy in
        // exchange for an O(W²) inner loop with no modulo. The samples are
        // written in the order they entered the ring, oldest first.
        let w = self.window;
        let split = self.write_idx;
        self.linear[..w - split].copy_from_slice(&self.buf[split..]);
        self.linear[w - split..].copy_from_slice(&self.buf[..split]);

        let w_half = w / 2;
        let max_tau = self.max_tau.min(w_half);
        self.cmnd[0] = 1.0;
        let lin = &self.linear[..w];
        let mut running_sum = 0.0f32;
        for tau in 1..=max_tau {
            // Inner loop: tight contiguous access pattern, autovectorizable.
            let mut d = 0.0f32;
            let head = &lin[..w_half];
            let tail = &lin[tau..tau + w_half];
            for j in 0..w_half {
                let diff = head[j] - tail[j];
                d += diff * diff;
            }
            running_sum += d;
            let cmnd = if running_sum > 0.0 {
                d * tau as f32 / running_sum
            } else {
                1.0
            };
            self.cmnd[tau] = cmnd;
        }

        let chosen_tau = pick_tau(&self.cmnd, self.min_tau, max_tau, self.threshold);
        let Some(tau) = chosen_tau else {
            return PitchEstimate::UNVOICED;
        };

        let a = self.cmnd.get(tau.saturating_sub(1)).copied().unwrap_or(self.cmnd[tau]);
        let b = self.cmnd[tau];
        let c = self.cmnd.get(tau + 1).copied().unwrap_or(b);
        let tau_refined = parabolic_interp(a, b, c) + tau as f32;

        PitchEstimate {
            hz: self.sample_rate / tau_refined,
            aperiodicity: b,
        }
    }
}

/// First τ in `[min_tau, max_tau)` whose CMND drops below `threshold`,
/// then walks down to the local minimum. Shared with the FFT-YIN variant.
pub(crate) fn pick_tau(cmnd: &[f32], min_tau: usize, max_tau: usize, threshold: f32) -> Option<usize> {
    let mut tau = min_tau;
    while tau < max_tau {
        if cmnd[tau] < threshold {
            while tau + 1 < max_tau && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            return Some(tau);
        }
        tau += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::TAU;

    fn detect_freq(sine_hz: f32, sr: f32) -> f32 {
        let mut y = YinDetector::new(sr, 60.0, 1000.0, 256);
        let n = y.window * 4;
        let mut last = PitchEstimate::UNVOICED;
        for i in 0..n {
            let t = i as f32 / sr;
            let x = (TAU * sine_hz * t).sin();
            if let Some(e) = y.push(x) {
                if e.is_voiced() {
                    last = e;
                }
            }
        }
        last.hz
    }

    #[test]
    fn detects_220hz_sine_within_1pct() {
        let hz = detect_freq(220.0, 48_000.0);
        let err = ((hz - 220.0) / 220.0).abs();
        assert!(err < 0.01, "expected ~220Hz, got {hz} (err={err})");
    }

    #[test]
    fn detects_110hz_sine_within_1pct() {
        let hz = detect_freq(110.0, 48_000.0);
        let err = ((hz - 110.0) / 110.0).abs();
        assert!(err < 0.01, "expected ~110Hz, got {hz} (err={err})");
    }

    #[test]
    fn silence_is_unvoiced() {
        let mut y = YinDetector::new(48_000.0, 60.0, 1000.0, 256);
        let mut last_voiced = false;
        for _ in 0..(y.window * 2) {
            if let Some(e) = y.push(0.0) {
                last_voiced = e.is_voiced();
            }
        }
        assert!(!last_voiced, "silence should not be flagged voiced");
    }
}
