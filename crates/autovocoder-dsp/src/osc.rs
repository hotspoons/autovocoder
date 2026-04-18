//! Anti-aliased sawtooth oscillator (PolyBLEP).
//!
//! A naive sawtooth ramp is a step discontinuity each cycle, which creates
//! aliasing above Nyquist. PolyBLEP adds a small polynomial correction near
//! the discontinuity — cheap and good enough for a vocoder carrier, where
//! upper harmonics get filtered by the band analysis anyway.

use core::f32::consts::TAU;

/// A single band-limited sawtooth oscillator.
pub struct Saw {
    sample_rate: f32,
    phase: f32, // in [0, 1)
    freq_hz: f32,
}

impl Saw {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            phase: 0.0,
            freq_hz: 0.0,
        }
    }

    pub fn set_frequency(&mut self, hz: f32) {
        self.freq_hz = hz.max(0.0);
    }

    pub fn reset_phase(&mut self) {
        self.phase = 0.0;
    }

    /// Emit one sample in [-1, 1].
    pub fn tick(&mut self) -> f32 {
        let dt = self.freq_hz / self.sample_rate; // phase increment
                                                  // Naive saw: 2*phase - 1
        let mut y = 2.0 * self.phase - 1.0;
        // PolyBLEP correction at the wrap-around discontinuity.
        y -= poly_blep(self.phase, dt);

        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        y
    }
}

/// Polynomial BLEP correction for a unit step at t=0.
/// `t` is the normalized phase in [0,1), `dt` the per-sample phase increment.
fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// Sine oscillator. Useful for tests and optional pure-tone carriers.
pub struct Sine {
    sample_rate: f32,
    phase: f32,
    freq_hz: f32,
}

impl Sine {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            phase: 0.0,
            freq_hz: 0.0,
        }
    }

    pub fn set_frequency(&mut self, hz: f32) {
        self.freq_hz = hz.max(0.0);
    }

    pub fn tick(&mut self) -> f32 {
        let y = (self.phase * TAU).sin();
        self.phase += self.freq_hz / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saw_is_bounded() {
        let mut s = Saw::new(48_000.0);
        s.set_frequency(220.0);
        for _ in 0..48_000 {
            let y = s.tick();
            assert!((-1.5..=1.5).contains(&y), "saw out of range: {y}");
        }
    }

    #[test]
    fn saw_zero_freq_is_dc() {
        let mut s = Saw::new(48_000.0);
        s.set_frequency(0.0);
        let y0 = s.tick();
        let y1 = s.tick();
        assert!((y0 - y1).abs() < 1e-6);
    }

    #[test]
    fn sine_is_bounded() {
        let mut s = Sine::new(48_000.0);
        s.set_frequency(440.0);
        for _ in 0..4800 {
            let y = s.tick();
            assert!(y.abs() <= 1.0 + 1e-5);
        }
    }
}
