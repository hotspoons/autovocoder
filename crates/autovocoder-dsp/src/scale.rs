//! Pitch quantization to musical scales and an exponential portamento
//! smoother for the carrier pitch.

/// A musical scale as a bitmask over the 12 pitch classes (C=0, C#=1, ...).
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    mask: u16,
}

impl Scale {
    pub fn from_pitch_classes(classes: &[u8]) -> Self {
        let mut mask = 0u16;
        for &c in classes {
            mask |= 1 << (c % 12);
        }
        Self { mask }
    }

    pub const CHROMATIC: Self = Self { mask: 0x0FFF };

    pub fn major(root_pc: u8) -> Self {
        // C major intervals: 0 2 4 5 7 9 11
        Self::from_pitch_classes_rotated(root_pc, &[0, 2, 4, 5, 7, 9, 11])
    }

    pub fn minor(root_pc: u8) -> Self {
        // Natural minor: 0 2 3 5 7 8 10
        Self::from_pitch_classes_rotated(root_pc, &[0, 2, 3, 5, 7, 8, 10])
    }

    pub fn dorian(root_pc: u8) -> Self {
        // Minor with raised 6: 0 2 3 5 7 9 10. Smooth, jazzy.
        Self::from_pitch_classes_rotated(root_pc, &[0, 2, 3, 5, 7, 9, 10])
    }

    pub fn phrygian(root_pc: u8) -> Self {
        // Minor with lowered 2: 0 1 3 5 7 8 10. Spanish / metal.
        Self::from_pitch_classes_rotated(root_pc, &[0, 1, 3, 5, 7, 8, 10])
    }

    pub fn lydian(root_pc: u8) -> Self {
        // Major with raised 4: 0 2 4 6 7 9 11. Dreamy / sci-fi.
        Self::from_pitch_classes_rotated(root_pc, &[0, 2, 4, 6, 7, 9, 11])
    }

    pub fn mixolydian(root_pc: u8) -> Self {
        // Major with lowered 7: 0 2 4 5 7 9 10. Bluesy / classic rock.
        Self::from_pitch_classes_rotated(root_pc, &[0, 2, 4, 5, 7, 9, 10])
    }

    pub fn harmonic_minor(root_pc: u8) -> Self {
        // Minor with raised 7: 0 2 3 5 7 8 11. Eastern / dramatic.
        Self::from_pitch_classes_rotated(root_pc, &[0, 2, 3, 5, 7, 8, 11])
    }

    pub fn major_pentatonic(root_pc: u8) -> Self {
        // 0 2 4 7 9. Always-safe melodies.
        Self::from_pitch_classes_rotated(root_pc, &[0, 2, 4, 7, 9])
    }

    pub fn minor_pentatonic(root_pc: u8) -> Self {
        // 0 3 5 7 10. Rock / blues lead bedrock.
        Self::from_pitch_classes_rotated(root_pc, &[0, 3, 5, 7, 10])
    }

    pub fn blues(root_pc: u8) -> Self {
        // Minor pentatonic + blue note: 0 3 5 6 7 10.
        Self::from_pitch_classes_rotated(root_pc, &[0, 3, 5, 6, 7, 10])
    }

    fn from_pitch_classes_rotated(root: u8, intervals: &[u8]) -> Self {
        let mut mask = 0u16;
        for &i in intervals {
            mask |= 1 << ((root + i) % 12);
        }
        Self { mask }
    }

    /// LV2 port value → scale at the given root pitch class.
    /// 0=Chromatic, 1=Major, 2=Minor, 3=Dorian, 4=Phrygian, 5=Lydian,
    /// 6=Mixolydian, 7=Harmonic Minor, 8=Major Pentatonic,
    /// 9=Minor Pentatonic, 10=Blues. Out-of-range falls back to Chromatic.
    pub fn from_int(scale_kind: i32, root_pc: u8) -> Self {
        match scale_kind {
            1 => Self::major(root_pc),
            2 => Self::minor(root_pc),
            3 => Self::dorian(root_pc),
            4 => Self::phrygian(root_pc),
            5 => Self::lydian(root_pc),
            6 => Self::mixolydian(root_pc),
            7 => Self::harmonic_minor(root_pc),
            8 => Self::major_pentatonic(root_pc),
            9 => Self::minor_pentatonic(root_pc),
            10 => Self::blues(root_pc),
            _ => Self::CHROMATIC,
        }
    }

    pub fn contains(&self, pc: u8) -> bool {
        (self.mask >> (pc % 12)) & 1 == 1
    }
}

/// Convert Hz → MIDI note number (float, A4=69=440Hz).
pub fn hz_to_midi(hz: f32) -> f32 {
    if hz <= 0.0 {
        return 0.0;
    }
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// Convert MIDI note number → Hz.
pub fn midi_to_hz(n: f32) -> f32 {
    440.0 * ((n - 69.0) / 12.0).exp2()
}

/// Snap a Hz value to the nearest note in `scale`.
pub fn quantize_hz_to_scale(hz: f32, scale: Scale) -> f32 {
    if hz <= 0.0 {
        return 0.0;
    }
    let midi = hz_to_midi(hz).round() as i32;
    // Search outward for a scale-allowed pitch class, max ±6 semitones.
    for delta in 0..=6i32 {
        for sign in [0i32, -1, 1] {
            if sign == 0 && delta != 0 {
                continue;
            }
            let cand = midi + sign * delta;
            if cand < 0 {
                continue;
            }
            let pc = (cand.rem_euclid(12)) as u8;
            if scale.contains(pc) {
                return midi_to_hz(cand as f32);
            }
        }
    }
    hz
}

/// Exponential one-pole portamento for carrier pitch (smooths in the log
/// domain so the glide sounds musical at all octaves).
pub struct Portamento {
    coeff: f32, // smoothing in log2(Hz) domain
    current_log2: f32,
}

impl Portamento {
    pub fn new(sample_rate: f32, time_ms: f32) -> Self {
        let t_sec = (time_ms.max(0.01)) * 1e-3;
        let coeff = 1.0 - (-1.0 / (sample_rate * t_sec)).exp();
        Self {
            coeff,
            current_log2: 0.0,
        }
    }

    pub fn set_time(&mut self, sample_rate: f32, time_ms: f32) {
        let t_sec = time_ms.max(0.01) * 1e-3;
        self.coeff = 1.0 - (-1.0 / (sample_rate * t_sec)).exp();
    }

    /// Feed a target Hz each sample, get the smoothed Hz.
    /// Negative / zero targets pass through (oscillator silence).
    pub fn process(&mut self, target_hz: f32) -> f32 {
        if target_hz <= 0.0 {
            return 0.0;
        }
        let target = target_hz.log2();
        if self.current_log2 == 0.0 {
            self.current_log2 = target;
        } else {
            self.current_log2 += self.coeff * (target - self.current_log2);
        }
        self.current_log2.exp2()
    }

    pub fn reset(&mut self) {
        self.current_log2 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_roundtrip() {
        for note in 20..100 {
            let hz = midi_to_hz(note as f32);
            let back = hz_to_midi(hz);
            assert!((back - note as f32).abs() < 1e-3);
        }
    }

    #[test]
    fn quantize_to_c_major_snaps_c_sharp_to_c() {
        // C major is 0,2,4,5,7,9,11. C#=1 is NOT in scale; should go to C=0 or D=2.
        let c_major = Scale::major(0);
        let c_sharp = midi_to_hz(61.0);
        let q = quantize_hz_to_scale(c_sharp, c_major);
        let q_midi = hz_to_midi(q).round() as i32;
        assert!(
            q_midi == 60 || q_midi == 62,
            "expected 60 or 62, got {q_midi}"
        );
    }

    #[test]
    fn portamento_converges() {
        let mut p = Portamento::new(48_000.0, 20.0);
        for _ in 0..2400 {
            // 50ms — should be well-converged
            let _ = p.process(440.0);
        }
        let y = p.process(440.0);
        assert!((y - 440.0).abs() < 1.0);
    }
}
