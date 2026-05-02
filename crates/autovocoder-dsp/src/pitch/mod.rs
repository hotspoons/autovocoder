//! Monophonic pitch detection.
//!
//! Three algorithms are exposed via a single enum-dispatch front (no vtable
//! call on the per-sample `push`):
//!
//! - [`PitchAlgorithm::YinClassic`] — classic time-domain YIN with a linear
//!   scratch buffer. No FFT dep, no allocations after construction. Best
//!   when CPU is plentiful or for very short windows.
//!
//! - [`PitchAlgorithm::YinFft`] — same YIN normalization, but the
//!   difference function is built from the autocorrelation computed via FFT.
//!   `O(W log W)` per estimate vs `O(W²)`. Same accuracy as classic, much
//!   cheaper on the speech-range windows we use (~2k samples).
//!
//! - [`PitchAlgorithm::FftPeak`] — Harmonic Product Spectrum. Frequency-
//!   domain peak picker; cheap and well-suited to sustained, periodic signals.
//!   Less robust on breathy / noisy material than the YIN variants.
//!
//! Reference for YIN: de Cheveigné & Kawahara 2002.
//! Reference for HPS: Schroeder 1968.

mod fft_peak;
mod yin_classic;
mod yin_fft;

pub use fft_peak::FftPeakDetector;
pub use yin_classic::YinDetector;
pub use yin_fft::YinFftDetector;

/// Algorithm selector — exposed as a host-facing parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PitchAlgorithm {
    YinClassic,
    YinFft,
    FftPeak,
}

impl PitchAlgorithm {
    /// LV2 port value → algorithm. Out-of-range clamps to `YinFft` (the
    /// default — best CPU/quality balance for our window sizes).
    pub fn from_int(i: i32) -> Self {
        match i {
            0 => PitchAlgorithm::YinClassic,
            2 => PitchAlgorithm::FftPeak,
            _ => PitchAlgorithm::YinFft,
        }
    }
}

/// Result of a pitch estimate.
#[derive(Clone, Copy, Debug)]
pub struct PitchEstimate {
    /// Estimated frequency in Hz, or 0.0 if no confident pitch.
    pub hz: f32,
    /// Normalized aperiodicity (lower = more tonal). For HPS this is a
    /// proxy derived from spectral peakiness rather than YIN's d'(τ).
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

/// Front for all pitch detectors. Enum dispatch — the per-sample `push`
/// usually does no work beyond a buffer write, so a vtable indirection
/// per sample would be wasteful. The match below compiles to a tag-test
/// and direct call.
pub enum PitchDetector {
    YinClassic(YinDetector),
    YinFft(YinFftDetector),
    FftPeak(FftPeakDetector),
}

impl PitchDetector {
    pub fn new(algo: PitchAlgorithm, sample_rate: f32, min_hz: f32, max_hz: f32, hop: usize) -> Self {
        match algo {
            PitchAlgorithm::YinClassic => {
                PitchDetector::YinClassic(YinDetector::new(sample_rate, min_hz, max_hz, hop))
            }
            PitchAlgorithm::YinFft => {
                PitchDetector::YinFft(YinFftDetector::new(sample_rate, min_hz, max_hz, hop))
            }
            PitchAlgorithm::FftPeak => {
                PitchDetector::FftPeak(FftPeakDetector::new(sample_rate, min_hz, max_hz, hop))
            }
        }
    }

    pub fn algorithm(&self) -> PitchAlgorithm {
        match self {
            PitchDetector::YinClassic(_) => PitchAlgorithm::YinClassic,
            PitchDetector::YinFft(_) => PitchAlgorithm::YinFft,
            PitchDetector::FftPeak(_) => PitchAlgorithm::FftPeak,
        }
    }

    #[inline]
    pub fn push(&mut self, x: f32) -> Option<PitchEstimate> {
        match self {
            PitchDetector::YinClassic(d) => d.push(x),
            PitchDetector::YinFft(d) => d.push(x),
            PitchDetector::FftPeak(d) => d.push(x),
        }
    }
}

/// Sub-sample interpolation around a parabolic minimum at (-1, a), (0, b),
/// (1, c). Returns the offset to add to the integer index. Used by YIN-
/// family detectors. Shared so the two YIN variants stay bit-identical at
/// the interpolation step.
pub(crate) fn parabolic_interp(a: f32, b: f32, c: f32) -> f32 {
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-9 {
        0.0
    } else {
        0.5 * (a - c) / denom
    }
}
