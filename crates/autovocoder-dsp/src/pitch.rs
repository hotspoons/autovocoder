//! Monophonic pitch detector (YIN algorithm).
//!
//! Reference: "YIN, a fundamental frequency estimator for speech and music",
//! de Cheveigné & Kawahara, 2002.
//!
//! Pipeline: difference function d(tau), cumulative-mean-normalized
//! difference d'(tau), absolute threshold pick, parabolic interpolation
//! around the minimum. No FFT — O(N*W) per estimate, but we only run this
//! every `hop` samples, so average cost is O(N*W / hop).

/// Result of a pitch estimate.
#[derive(Clone, Copy, Debug)]
pub struct PitchEstimate {
    /// Estimated frequency in Hz, or 0.0 if no confident pitch.
    pub hz: f32,
    /// Normalized aperiodicity from YIN (lower = more tonal).
    pub aperiodicity: f32,
}

impl PitchEstimate {
    pub const UNVOICED: Self = Self {
        hz: 0.0,
        aperiodicity: 1.0,
    };

    pub fn is_voiced(&self) -> bool {
        self.hz > 0.0
    }
}

/// YIN pitch detector. Maintains a ring buffer and emits an estimate every
/// `hop` samples once the window is full.
pub struct YinDetector {
    sample_rate: f32,
    window: usize,  // analysis window size (samples)
    hop: usize,     // samples between estimates
    threshold: f32, // YIN absolute threshold (0.10–0.20 typical)
    buf: Vec<f32>,  // circular buffer of size `window`
    write_idx: usize,
    samples_since_hop: usize,
    scratch: Vec<f32>, // d'(tau) scratch, reused
    min_tau: usize,
    max_tau: usize,
}

impl YinDetector {
    /// `min_hz`/`max_hz` bound the search space and set the buffer size.
    pub fn new(sample_rate: f32, min_hz: f32, max_hz: f32, hop: usize) -> Self {
        let min_tau = (sample_rate / max_hz).floor() as usize;
        let max_tau = (sample_rate / min_hz).ceil() as usize;
        // Window must be at least 2*max_tau for a valid difference function.
        let window = (2 * max_tau).next_power_of_two();
        Self {
            sample_rate,
            window,
            hop: hop.max(1),
            threshold: 0.15,
            buf: vec![0.0; window],
            write_idx: 0,
            samples_since_hop: 0,
            scratch: vec![0.0; max_tau + 1],
            min_tau: min_tau.max(2),
            max_tau,
        }
    }

    pub fn set_threshold(&mut self, t: f32) {
        self.threshold = t.clamp(0.01, 0.5);
    }

    /// Feed one sample; optionally returns an estimate on hop boundaries.
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

    fn estimate(&mut self) -> PitchEstimate {
        // Copy the ring buffer into a linear view ending at write_idx.
        // For simplicity, materialize into scratch-adjacent storage — but
        // we can also read via modular index. Use closure for clarity.
        let w = self.window;
        let idx = |i: usize| self.buf[(self.write_idx + i) % w];

        // YIN step 2: difference function d(tau) for tau in [1, max_tau].
        // d(tau) = sum_{j=0}^{W-tau-1} (x[j] - x[j+tau])^2
        // For speed we only compute tau in [min_tau, max_tau].
        let w_half = w / 2; // integrate over half the window
        let max_tau = self.max_tau.min(w_half);

        // d'(0) = 1 by YIN convention; d'[tau] = d[tau] / ((1/tau)*sum d[1..=tau])
        self.scratch.fill(0.0);
        self.scratch[0] = 1.0;

        let mut running_sum = 0.0f32;
        for tau in 1..=max_tau {
            let mut d = 0.0f32;
            for j in 0..w_half {
                let diff = idx(j) - idx(j + tau);
                d += diff * diff;
            }
            running_sum += d;
            let cmnd = if running_sum > 0.0 {
                d * tau as f32 / running_sum
            } else {
                1.0
            };
            self.scratch[tau] = cmnd;
        }

        // YIN step 4: pick the first tau below threshold that's a local min.
        let mut chosen_tau: Option<usize> = None;
        let mut tau = self.min_tau;
        while tau < max_tau {
            if self.scratch[tau] < self.threshold {
                // walk down the local minimum
                while tau + 1 < max_tau && self.scratch[tau + 1] < self.scratch[tau] {
                    tau += 1;
                }
                chosen_tau = Some(tau);
                break;
            }
            tau += 1;
        }

        let Some(tau) = chosen_tau else {
            return PitchEstimate::UNVOICED;
        };

        // Parabolic interpolation around the minimum for sub-sample accuracy.
        let tau_refined = parabolic_interp(
            self.scratch
                .get(tau.saturating_sub(1))
                .copied()
                .unwrap_or(self.scratch[tau]),
            self.scratch[tau],
            self.scratch
                .get(tau + 1)
                .copied()
                .unwrap_or(self.scratch[tau]),
        ) + tau as f32;

        let hz = self.sample_rate / tau_refined;
        PitchEstimate {
            hz,
            aperiodicity: self.scratch[tau],
        }
    }
}

/// Returns the sub-sample offset of the minimum of the parabola fit through
/// (-1, a), (0, b), (1, c). Caller adds this to the integer index.
fn parabolic_interp(a: f32, b: f32, c: f32) -> f32 {
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-9 {
        0.0
    } else {
        0.5 * (a - c) / denom
    }
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
