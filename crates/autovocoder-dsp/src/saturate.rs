//! Stateless waveshaping saturator with three modes.
//!
//! Tube, tape, and fuzz are all variations on the same theme — pre-gain the
//! signal, run it through `tanh`, optionally compensate makeup. They differ
//! in pre-gain magnitude (how hard the soft-knee is hit) and in whether
//! they're symmetric:
//!
//! - **Tube** — moderate drive with a small asymmetric bias term so even
//!   harmonics show up. Warm, slightly fuzzed-up sound; maps to "tape head"
//!   or "preamp" character on a real channel strip.
//! - **Tape** — symmetric soft-clip via `tanh(g·x)`. Pure odd harmonics,
//!   smooth; rounds peaks rather than crushing them.
//! - **Fuzz** — same shape as tape but with much higher pre-gain, so
//!   anything past whisper-quiet hits the rails of `tanh`. Effectively a
//!   smoothed hard-clipper.
//!
//! Output makeup is a simple `1 / (1 + drive)` attenuation so the loudness
//! stays roughly constant as drive goes up — perceived amount of saturation
//! changes but not the level. Caller can still trim with output gain.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveMode {
    Tube,
    Tape,
    Fuzz,
}

impl DriveMode {
    /// LV2 port value → mode. Out-of-range clamps to Tape (most neutral).
    pub fn from_int(i: i32) -> Self {
        match i {
            0 => DriveMode::Tube,
            2 => DriveMode::Fuzz,
            _ => DriveMode::Tape,
        }
    }
}

pub struct Saturator {
    enabled: bool,
    mode: DriveMode,
    drive: f32, // 0..1, normalized
}

impl Saturator {
    pub fn new() -> Self {
        Self {
            enabled: false,
            mode: DriveMode::Tape,
            drive: 0.5,
        }
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_mode(&mut self, mode: DriveMode) {
        self.mode = mode;
    }

    pub fn set_drive(&mut self, drive_0_1: f32) {
        self.drive = drive_0_1.clamp(0.0, 1.0);
    }

    #[inline]
    pub fn process_sample(&self, x: f32) -> f32 {
        if self.enabled {
            self.shape(x)
        } else {
            x
        }
    }

    pub fn process_block(&self, buf: &mut [f32]) {
        if !self.enabled {
            return;
        }
        for s in buf.iter_mut() {
            *s = self.shape(*s);
        }
    }

    #[inline(always)]
    fn shape(&self, x: f32) -> f32 {
        // Per-mode pre-gain. Tube/Tape sit in a warm range; Fuzz is much
        // hotter so even modest signals slam the soft-clip.
        let drive = self.drive;
        let pre_gain = match self.mode {
            DriveMode::Tube => 1.0 + drive * 5.0,
            DriveMode::Tape => 1.0 + drive * 4.0,
            DriveMode::Fuzz => 1.0 + drive * 24.0,
        };
        let pre = pre_gain * x;
        let shaped = match self.mode {
            // Asymmetric bias term: positive halves get pushed slightly
            // harder than negative halves. The asymmetry yields second
            // harmonic content — what people call "tube warmth."
            DriveMode::Tube => (pre + 0.18 * drive * x.abs()).tanh(),
            DriveMode::Tape | DriveMode::Fuzz => pre.tanh(),
        };
        // Loudness compensation. Without this, drive=1 sounds dramatically
        // louder than drive=0 — useful for "amount" feel but bad for A/B.
        let comp = 1.0 / (1.0 + drive * 1.4);
        shaped * comp
    }
}

impl Default for Saturator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_is_identity() {
        let s = Saturator::new();
        for x in [-0.9, -0.3, 0.0, 0.3, 0.9] {
            assert!((s.process_sample(x) - x).abs() < 1e-9);
        }
    }

    #[test]
    fn enabled_clips_extremes() {
        for mode in [DriveMode::Tube, DriveMode::Tape, DriveMode::Fuzz] {
            let mut s = Saturator::new();
            s.set_enabled(true);
            s.set_mode(mode);
            s.set_drive(1.0);
            // Hot input must come out below unity (the tanh ceiling × comp).
            let y = s.process_sample(2.0);
            assert!(y.abs() < 1.0, "{mode:?} clip overshot at 2.0: {y}");
            let y = s.process_sample(-2.0);
            assert!(y.abs() < 1.0, "{mode:?} clip overshot at -2.0: {y}");
        }
    }

    #[test]
    fn fuzz_more_aggressive_than_tape() {
        // For the same modest input, fuzz should be close to ±1 (slammed)
        // while tape should still have a lot of dynamic range.
        let mut tape = Saturator::new();
        tape.set_enabled(true);
        tape.set_mode(DriveMode::Tape);
        tape.set_drive(1.0);
        let mut fuzz = Saturator::new();
        fuzz.set_enabled(true);
        fuzz.set_mode(DriveMode::Fuzz);
        fuzz.set_drive(1.0);
        // Apply makeup compensation already built in.
        let y_tape = tape.process_sample(0.3);
        let y_fuzz = fuzz.process_sample(0.3);
        assert!(
            y_fuzz.abs() > y_tape.abs(),
            "fuzz {y_fuzz} should be more saturated than tape {y_tape}"
        );
    }

    #[test]
    fn block_matches_per_sample() {
        let mut s = Saturator::new();
        s.set_enabled(true);
        s.set_mode(DriveMode::Tape);
        s.set_drive(0.6);
        let n = 256;
        let input: Vec<f32> = (0..n).map(|i| 0.5 * (i as f32 * 0.04).sin()).collect();
        let per: Vec<f32> = input.iter().map(|&x| s.process_sample(x)).collect();
        let mut blk = input.clone();
        s.process_block(&mut blk);
        for (i, (a, b)) in per.iter().zip(blk.iter()).enumerate() {
            assert!((a - b).abs() < 1e-7, "drift at {i}: {a} vs {b}");
        }
    }
}
