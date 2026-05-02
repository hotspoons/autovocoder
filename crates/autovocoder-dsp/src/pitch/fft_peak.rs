//! Frequency-domain pitch detector using the Harmonic Product Spectrum.
//!
//! HPS multiplies the magnitude spectrum by downsampled copies of itself
//! (×2, ×3, ×4, …). Harmonics of the true fundamental line up at the
//! fundamental's bin in every downsampled copy, so they reinforce; spurious
//! peaks (from inharmonic content or noise) do not. Peak-pick the product
//! and you have f0.
//!
//! Properties:
//!   - Cheapest of the three detectors per estimate (one FFT, no IFFT).
//!   - Robust on sustained, harmonically-rich signals (sung vowels, brass).
//!   - Less robust than YIN on breathy / noisy / weakly periodic signals.
//!
//! We apply a Hann window before the FFT to keep spectral leakage from
//! polluting the lower harmonics, and pick the peak with parabolic
//! interpolation in the bin domain for sub-bin frequency resolution.

use std::sync::Arc;

use rustfft::{num_complex::Complex32, Fft, FftPlanner};

use super::PitchEstimate;

const HARMONICS: usize = 5;

pub struct FftPeakDetector {
    sample_rate: f32,
    window: usize,
    fft_size: usize,
    hop: usize,
    /// Ring buffer of recent samples.
    buf: Vec<f32>,
    write_idx: usize,
    samples_since_hop: usize,

    fwd: Arc<dyn Fft<f32>>,
    hann: Vec<f32>,
    cspec: Vec<Complex32>,
    mag: Vec<f32>,
    hps: Vec<f32>,

    min_bin: usize,
    max_bin: usize,
}

impl FftPeakDetector {
    pub fn new(sample_rate: f32, min_hz: f32, max_hz: f32, hop: usize) -> Self {
        // Window roughly matches what YIN uses for the same min_hz so the
        // detectors are comparable in latency. Round up to next power of two
        // for FFT efficiency.
        let max_tau = (sample_rate / min_hz).ceil() as usize;
        let window = (2 * max_tau).next_power_of_two();
        let fft_size = window;
        let half = fft_size / 2;

        let bin_to_hz = sample_rate / fft_size as f32;
        // Reserve enough bins above max_hz that HARMONICS-th harmonic is in
        // range — we read mag[bin * h] for h up to HARMONICS, so the
        // effective f0 search has to be limited to fft_size / (2*HARMONICS).
        let max_bin = ((max_hz / bin_to_hz) as usize).min(half / HARMONICS).max(2);
        let min_bin = ((min_hz / bin_to_hz) as usize).max(2);

        let mut planner = FftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(fft_size);

        // Periodic Hann (matches numpy.hanning's "symmetric" + 1 sample).
        let hann: Vec<f32> = (0..window)
            .map(|n| {
                let t = n as f32 / window as f32;
                0.5 - 0.5 * (core::f32::consts::TAU * t).cos()
            })
            .collect();

        Self {
            sample_rate,
            window,
            fft_size,
            hop: hop.max(1),
            buf: vec![0.0; window],
            write_idx: 0,
            samples_since_hop: 0,
            fwd,
            hann,
            cspec: vec![Complex32::default(); fft_size],
            mag: vec![0.0; half + 1],
            hps: vec![0.0; half / HARMONICS + 2],
            min_bin,
            max_bin,
        }
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

    // Index-form loops below stage data across `cspec`, `mag`, and `hps`
    // in parallel — clearer to read than zip-based iterators.
    #[allow(clippy::needless_range_loop)]
    fn estimate(&mut self) -> PitchEstimate {
        let w = self.window;
        let n = self.fft_size;
        let split = self.write_idx;

        // Snapshot ring → time-domain windowed buffer.
        let cspec = &mut self.cspec[..];
        let mut energy = 0.0f32;
        for i in 0..(w - split) {
            let v = self.buf[split + i] * self.hann[i];
            cspec[i].re = v;
            cspec[i].im = 0.0;
            energy += v * v;
        }
        for i in 0..split {
            let v = self.buf[i] * self.hann[(w - split) + i];
            cspec[(w - split) + i].re = v;
            cspec[(w - split) + i].im = 0.0;
            energy += v * v;
        }
        for i in w..n {
            cspec[i].re = 0.0;
            cspec[i].im = 0.0;
        }

        // Cheap silence gate — skip the FFT altogether on quiet input.
        if energy < 1e-6 {
            return PitchEstimate::UNVOICED;
        }

        self.fwd.process(cspec);

        // Magnitude spectrum (one-sided).
        let half = n / 2;
        for k in 0..=half {
            let c = cspec[k];
            self.mag[k] = (c.re * c.re + c.im * c.im).sqrt();
        }

        // Harmonic Product Spectrum: hps[k] = Π_{h=1..HARMONICS} mag[k*h].
        let upper = self.max_bin.min(half / HARMONICS);
        let mag = &self.mag;
        let hps = &mut self.hps;
        for k in 0..=upper {
            let mut p = mag[k];
            for h in 2..=HARMONICS {
                p *= mag[k * h];
            }
            hps[k] = p;
        }

        // Pick the strongest peak in [min_bin, upper].
        let mut best = 0usize;
        let mut best_val = 0.0f32;
        for k in self.min_bin..=upper {
            if hps[k] > best_val {
                best_val = hps[k];
                best = k;
            }
        }
        if best == 0 || best_val == 0.0 {
            return PitchEstimate::UNVOICED;
        }

        // Voicedness: compare HPS peak to total spectrum energy. Tunable.
        let mut total: f32 = 0.0;
        for v in &self.mag[..=half] {
            total += v;
        }
        let peakiness = if total > 0.0 {
            mag[best] / (total / half as f32)
        } else {
            0.0
        };
        if peakiness < 3.0 {
            return PitchEstimate::UNVOICED;
        }

        // Parabolic interpolation in the magnitude (not HPS) domain — the
        // HPS shape is multiplicative-noisy near the peak, and the
        // fundamental's main lobe in `mag` is typically well-formed.
        let bin_refined = if best > 0 && best < half {
            let a = mag[best - 1];
            let b = mag[best];
            let c = mag[best + 1];
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-9 {
                best as f32 + 0.5 * (a - c) / denom
            } else {
                best as f32
            }
        } else {
            best as f32
        };
        let bin_to_hz = self.sample_rate / n as f32;
        PitchEstimate {
            hz: bin_refined * bin_to_hz,
            aperiodicity: 1.0 / peakiness.max(1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::TAU;

    fn detect_partials(f0: f32, sr: f32) -> f32 {
        let mut d = FftPeakDetector::new(sr, 60.0, 1000.0, 256);
        let n = d.window * 4;
        let mut last = PitchEstimate::UNVOICED;
        for i in 0..n {
            let t = i as f32 / sr;
            // Real-ish: sum of 6 partials so HPS has something to lock onto.
            let mut x = 0.0;
            for h in 1..=6 {
                x += (TAU * f0 * h as f32 * t).sin() / h as f32;
            }
            if let Some(e) = d.push(0.3 * x) {
                if e.is_voiced() {
                    last = e;
                }
            }
        }
        last.hz
    }

    #[test]
    fn detects_220hz_within_2pct() {
        let hz = detect_partials(220.0, 48_000.0);
        let err = ((hz - 220.0) / 220.0).abs();
        assert!(err < 0.02, "expected ~220Hz, got {hz} (err={err})");
    }

    #[test]
    fn detects_440hz_within_2pct() {
        let hz = detect_partials(440.0, 48_000.0);
        let err = ((hz - 440.0) / 440.0).abs();
        assert!(err < 0.02, "expected ~440Hz, got {hz} (err={err})");
    }

    #[test]
    fn silence_is_unvoiced() {
        let mut d = FftPeakDetector::new(48_000.0, 60.0, 1000.0, 256);
        let mut last_voiced = false;
        for _ in 0..(d.window * 2) {
            if let Some(e) = d.push(0.0) {
                last_voiced = e.is_voiced();
            }
        }
        assert!(!last_voiced, "silence should not be flagged voiced");
    }
}
