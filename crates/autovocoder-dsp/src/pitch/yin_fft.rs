//! YIN with the difference function derived from the autocorrelation via FFT.
//!
//! YIN's classic difference function
//!     d(τ) = Σ_{j=0..W-τ-1} (x[j] - x[j+τ])²
//! expands to
//!     d(τ) = Σ x[j]² + Σ x[j+τ]² − 2·r(τ)
//! where `r(τ)` is the (one-sided) autocorrelation. `r(τ)` is the inverse
//! FFT of |X(f)|² when `x` is zero-padded to `2W`. That makes the whole
//! difference function `O(W log W)` instead of `O(W²)`.
//!
//! Once `d(τ)` is in hand, the rest of YIN is identical to the classic
//! implementation: cumulative-mean normalization, threshold pick, parabolic
//! interpolation. We share the pick step and the interpolation helper.

use std::sync::Arc;

use rustfft::{num_complex::Complex32, Fft, FftPlanner};

use super::yin_classic::pick_tau;
use super::{parabolic_interp, PitchEstimate};

pub struct YinFftDetector {
    sample_rate: f32,
    window: usize,
    fft_size: usize, // 2 * window, power of two
    hop: usize,
    threshold: f32,
    /// Ring buffer holding the most recent `window` input samples.
    buf: Vec<f32>,
    write_idx: usize,
    samples_since_hop: usize,

    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    /// Linear-order snapshot of the ring buffer (oldest sample first).
    linear: Vec<f32>,
    /// Complex scratch for the FFT round-trip. Holds spectrum then ACF.
    cspec: Vec<Complex32>,
    /// Prefix sum of x²: `prefix[k] = Σ_{j=0..k} x[j]²`. Length `window+1`.
    prefix_sq: Vec<f32>,
    /// d'(τ) scratch.
    cmnd: Vec<f32>,

    min_tau: usize,
    max_tau: usize,
}

impl YinFftDetector {
    pub fn new(sample_rate: f32, min_hz: f32, max_hz: f32, hop: usize) -> Self {
        let min_tau = (sample_rate / max_hz).floor() as usize;
        let max_tau = (sample_rate / min_hz).ceil() as usize;
        let window = (2 * max_tau).next_power_of_two();
        // Zero-pad to 2W so circular convolution from the FFT equals linear
        // autocorrelation up to lag W-1.
        let fft_size = (2 * window).next_power_of_two();

        let mut planner = FftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(fft_size);
        let inv = planner.plan_fft_inverse(fft_size);

        Self {
            sample_rate,
            window,
            fft_size,
            hop: hop.max(1),
            threshold: 0.15,
            buf: vec![0.0; window],
            write_idx: 0,
            samples_since_hop: 0,
            fwd,
            inv,
            linear: vec![0.0; window],
            cspec: vec![Complex32::default(); fft_size],
            prefix_sq: vec![0.0; window + 1],
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

    // Index-form loops below run over multiple parallel buffers (`lin`,
    // `cspec`, `prefix_sq`) — explicit indexing reads more clearly than
    // chained `iter().zip(...)` would.
    #[allow(clippy::needless_range_loop)]
    fn estimate(&mut self) -> PitchEstimate {
        let w = self.window;
        let n = self.fft_size;

        // Snapshot the ring → linear buffer, oldest first.
        let split = self.write_idx;
        self.linear[..w - split].copy_from_slice(&self.buf[split..]);
        self.linear[w - split..].copy_from_slice(&self.buf[..split]);
        let lin = &self.linear[..w];

        // Stage the time-domain FFT input from the linear buffer, zero-padded.
        let cspec = &mut self.cspec[..];
        for (i, &v) in lin.iter().enumerate() {
            cspec[i].re = v;
            cspec[i].im = 0.0;
        }
        for c in &mut cspec[w..] {
            c.re = 0.0;
            c.im = 0.0;
        }

        // Power-spectrum = forward FFT × conjugate.
        self.fwd.process(cspec);
        for c in cspec.iter_mut() {
            let p = c.re * c.re + c.im * c.im;
            c.re = p;
            c.im = 0.0;
        }
        // Inverse FFT → autocorrelation, unscaled (rustfft inverse is /1).
        self.inv.process(cspec);

        // Prefix sum of x² over the linear window, so the two boundary
        // sums in d(τ) collapse to constant-time lookups:
        //   d(τ) = Σ_{j=0..W-τ} x[j]² + Σ_{j=τ..W} x[j]² − 2·r(τ)
        //        = prefix[W-τ] + (total − prefix[τ]) − 2·r(τ)
        self.prefix_sq[0] = 0.0;
        for j in 0..w {
            self.prefix_sq[j + 1] = self.prefix_sq[j] + lin[j] * lin[j];
        }
        let total = self.prefix_sq[w];

        let max_tau = self.max_tau.min(w / 2);
        let inv_n = 1.0 / n as f32;
        self.cmnd[0] = 1.0;
        let mut running_sum = 0.0f32;
        for tau in 1..=max_tau {
            let r = cspec[tau].re * inv_n;
            let head = self.prefix_sq[w - tau];
            let tail = total - self.prefix_sq[tau];
            // Numerical noise can push d slightly negative; clamp.
            let d = (head + tail - 2.0 * r).max(0.0);
            running_sum += d;
            let cmnd = if running_sum > 0.0 {
                d * tau as f32 / running_sum
            } else {
                1.0
            };
            self.cmnd[tau] = cmnd;
        }

        let chosen = pick_tau(&self.cmnd, self.min_tau, max_tau, self.threshold);
        let Some(tau) = chosen else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::TAU;

    fn detect_freq(sine_hz: f32, sr: f32) -> f32 {
        let mut y = YinFftDetector::new(sr, 60.0, 1000.0, 256);
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
        let mut y = YinFftDetector::new(48_000.0, 60.0, 1000.0, 256);
        let mut last_voiced = false;
        for _ in 0..(y.window * 2) {
            if let Some(e) = y.push(0.0) {
                last_voiced = e.is_voiced();
            }
        }
        assert!(!last_voiced, "silence should not be flagged voiced");
    }

    #[test]
    fn detects_440hz_sine() {
        let hz = detect_freq(440.0, 48_000.0);
        let err = ((hz - 440.0) / 440.0).abs();
        assert!(err < 0.01, "expected ~440Hz, got {hz} (err={err})");
    }
}
