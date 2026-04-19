//! Top-level autovocoder: pitch-detect the voice, quantize to scale,
//! synthesize a carrier, drive the vocoder.

use crate::dynamics::{db_to_linear, Compressor};
use crate::osc::Saw;
use crate::pitch::{PitchEstimate, YinDetector};
use crate::scale::{midi_to_hz, quantize_hz_to_scale, Portamento, Scale};
use crate::vocoder::{Vocoder, VocoderConfig};

/// Chord voicing — semitone intervals from the root. Up to 5 notes fit our
/// oscillator bank; longer patterns get truncated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordVoicing {
    Power,      // 1 - 5            (0, 7)
    Major,      // 1 - 3 - 5        (0, 4, 7)
    Minor,      // 1 - b3 - 5       (0, 3, 7)
    Sus2,       // 1 - 2 - 5        (0, 2, 7)
    Sus4,       // 1 - 4 - 5        (0, 5, 7)
    Diminished, // 1 - b3 - b5      (0, 3, 6)
    Augmented,  // 1 - 3 - #5       (0, 4, 8)
    Maj7,       // 1 - 3 - 5 - 7    (0, 4, 7, 11)
    Min7,       // 1 - b3 - 5 - b7  (0, 3, 7, 10)
    Dom7,       // 1 - 3 - 5 - b7   (0, 4, 7, 10)
    Dim7,       // 1 - b3 - b5 - bb7 (0, 3, 6, 9)
    HalfDim7,   // 1 - b3 - b5 - b7 (0, 3, 6, 10)
    Add9,       // 1 - 3 - 5 - 9    (0, 4, 7, 14)
    Dom9,       // 1 - 3 - 5 - b7 - 9 (0, 4, 7, 10, 14)
    Min9,       // 1 - b3 - 5 - b7 - 9 (0, 3, 7, 10, 14)
}

impl ChordVoicing {
    pub fn intervals(self) -> &'static [i8] {
        match self {
            ChordVoicing::Power => &[0, 7],
            ChordVoicing::Major => &[0, 4, 7],
            ChordVoicing::Minor => &[0, 3, 7],
            ChordVoicing::Sus2 => &[0, 2, 7],
            ChordVoicing::Sus4 => &[0, 5, 7],
            ChordVoicing::Diminished => &[0, 3, 6],
            ChordVoicing::Augmented => &[0, 4, 8],
            ChordVoicing::Maj7 => &[0, 4, 7, 11],
            ChordVoicing::Min7 => &[0, 3, 7, 10],
            ChordVoicing::Dom7 => &[0, 4, 7, 10],
            ChordVoicing::Dim7 => &[0, 3, 6, 9],
            ChordVoicing::HalfDim7 => &[0, 3, 6, 10],
            ChordVoicing::Add9 => &[0, 4, 7, 14],
            ChordVoicing::Dom9 => &[0, 4, 7, 10, 14],
            ChordVoicing::Min9 => &[0, 3, 7, 10, 14],
        }
    }

    /// LV2 port value → voicing. Out-of-range clamps to Major.
    pub fn from_int(i: i32) -> Self {
        use ChordVoicing::*;
        match i {
            0 => Power,
            1 => Major,
            2 => Minor,
            3 => Sus2,
            4 => Sus4,
            5 => Diminished,
            6 => Augmented,
            7 => Maj7,
            8 => Min7,
            9 => Dom7,
            10 => Dim7,
            11 => HalfDim7,
            12 => Add9,
            13 => Dom9,
            14 => Min9,
            _ => Major,
        }
    }
}

/// Carrier voicing mode.
#[derive(Clone, Copy, Debug)]
pub enum CarrierMode {
    /// One saw at the quantized pitch of the input voice.
    Mono,
    /// A chord whose root tracks the input voice, spelled by `voicing`.
    Chord(ChordVoicing),
    /// Single saw at a fixed MIDI note — ignores input pitch.
    Fixed { midi: u8 },
    /// A chord rooted at a fixed MIDI note — classic Soundwave sound.
    FixedChord { midi: u8, voicing: ChordVoicing },
}

impl CarrierMode {
    pub fn major_triad() -> Self {
        Self::Chord(ChordVoicing::Major)
    }
    pub fn minor_triad() -> Self {
        Self::Chord(ChordVoicing::Minor)
    }
}

/// Fixed capacity for the oscillator bank. Covers every chord up to 9ths.
const CARRIER_VOICES: usize = 5;

#[derive(Clone, Copy, Debug)]
pub struct AutoVocoderConfig {
    pub vocoder: VocoderConfig,
    pub scale: Scale,
    pub carrier_mode: CarrierMode,
    pub portamento_ms: f32,
    pub dry_wet: f32,       // 0.0 = only voice, 1.0 = only vocoded
    pub carrier_level: f32, // saw level fed to the vocoder
    pub pitch_min_hz: f32,
    pub pitch_max_hz: f32,
    // Input stage (pre-vocoder).
    pub input_gain_db: f32, // applied to the voice before everything
    // Output stage.
    pub compressor_enabled: bool,
    pub compressor_threshold_db: f32,
    pub output_gain_db: f32, // applied after the compressor (classic makeup)
}

impl Default for AutoVocoderConfig {
    fn default() -> Self {
        Self {
            vocoder: VocoderConfig::default(),
            scale: Scale::CHROMATIC,
            carrier_mode: CarrierMode::Mono,
            portamento_ms: 25.0,
            dry_wet: 1.0,
            carrier_level: 0.6,
            pitch_min_hz: 70.0,
            pitch_max_hz: 800.0,
            // Input gain: bring quiet vocals up to a level where the
            // envelope followers can do real work. +9 dB is a conservative
            // default for studio mic levels (~-18 dBFS typical).
            input_gain_db: 9.0,
            // Output stage defaults — enough post-gain to sit in a mix
            // without cranking anything. Users can push either gain to
            // +60 dB; compressor + final soft-clamp prevent clipping.
            compressor_enabled: true,
            compressor_threshold_db: -18.0,
            output_gain_db: 6.0,
        }
    }
}

pub struct AutoVocoder {
    sample_rate: f32,
    cfg: AutoVocoderConfig,
    yin: YinDetector,
    last_pitch: PitchEstimate,
    // Up to CARRIER_VOICES oscs. Unused slots stay at 0 Hz so they produce
    // no output. Enough to cover every chord voicing up through 9ths.
    oscs: [Saw; CARRIER_VOICES],
    portos: [Portamento; CARRIER_VOICES],
    vocoder: Vocoder,
    compressor: Compressor,
    input_gain: f32,  // linear
    output_gain: f32, // linear (post-compressor makeup)
}

impl AutoVocoder {
    pub fn new(sample_rate: f32, cfg: AutoVocoderConfig) -> Self {
        let yin = YinDetector::new(sample_rate, cfg.pitch_min_hz, cfg.pitch_max_hz, 256);
        let mut compressor = Compressor::new(sample_rate, cfg.compressor_threshold_db);
        compressor.set_enabled(cfg.compressor_enabled);
        // `[T; N]` from a non-Copy constructor — do it by hand.
        let oscs: [Saw; CARRIER_VOICES] = std::array::from_fn(|_| Saw::new(sample_rate));
        let portos: [Portamento; CARRIER_VOICES] =
            std::array::from_fn(|_| Portamento::new(sample_rate, cfg.portamento_ms));
        Self {
            sample_rate,
            yin,
            last_pitch: PitchEstimate::UNVOICED,
            oscs,
            portos,
            vocoder: Vocoder::new(sample_rate, cfg.vocoder),
            compressor,
            input_gain: db_to_linear(cfg.input_gain_db),
            output_gain: db_to_linear(cfg.output_gain_db),
            cfg,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn set_scale(&mut self, scale: Scale) {
        self.cfg.scale = scale;
    }

    pub fn set_carrier_mode(&mut self, mode: CarrierMode) {
        self.cfg.carrier_mode = mode;
    }

    pub fn set_dry_wet(&mut self, mix: f32) {
        self.cfg.dry_wet = mix.clamp(0.0, 1.0);
    }

    pub fn set_portamento_ms(&mut self, ms: f32) {
        let clamped = ms.clamp(0.5, 1000.0);
        self.cfg.portamento_ms = clamped;
        for p in &mut self.portos {
            p.set_time(self.sample_rate, clamped);
        }
    }

    pub fn set_carrier_level(&mut self, level: f32) {
        self.cfg.carrier_level = level.clamp(0.0, 2.0);
    }

    pub fn set_input_gain_db(&mut self, db: f32) {
        let clamped = db.clamp(-20.0, 60.0);
        self.cfg.input_gain_db = clamped;
        self.input_gain = db_to_linear(clamped);
    }

    pub fn set_output_gain_db(&mut self, db: f32) {
        let clamped = db.clamp(-20.0, 60.0);
        self.cfg.output_gain_db = clamped;
        self.output_gain = db_to_linear(clamped);
    }

    pub fn set_compressor_enabled(&mut self, on: bool) {
        self.cfg.compressor_enabled = on;
        self.compressor.set_enabled(on);
    }

    pub fn set_compressor_threshold_db(&mut self, db: f32) {
        self.cfg.compressor_threshold_db = db;
        self.compressor.set_threshold_db(db);
    }

    pub fn reset(&mut self) {
        self.vocoder.reset();
        self.compressor.reset();
        self.last_pitch = PitchEstimate::UNVOICED;
        for o in &mut self.oscs {
            o.reset_phase();
        }
        for p in &mut self.portos {
            p.reset();
        }
    }

    /// Target Hz for each carrier oscillator given the latest voice pitch.
    /// Unused slots return 0.0 (silenced).
    fn carrier_targets(&self, voice_hz: f32) -> [f32; CARRIER_VOICES] {
        let mut out = [0.0f32; CARRIER_VOICES];
        let (root_hz, voicing) = match self.cfg.carrier_mode {
            CarrierMode::Mono => (quantize_hz_to_scale(voice_hz, self.cfg.scale), None),
            CarrierMode::Chord(v) => (quantize_hz_to_scale(voice_hz, self.cfg.scale), Some(v)),
            CarrierMode::Fixed { midi } => (midi_to_hz(midi as f32), None),
            CarrierMode::FixedChord { midi, voicing } => (midi_to_hz(midi as f32), Some(voicing)),
        };
        if root_hz <= 0.0 {
            return out;
        }
        match voicing {
            None => {
                out[0] = root_hz;
            }
            Some(v) => {
                let intervals = v.intervals();
                for (i, &semis) in intervals.iter().enumerate().take(CARRIER_VOICES) {
                    out[i] = root_hz * 2f32.powf(semis as f32 / 12.0);
                }
            }
        }
        out
    }

    /// Process one sample. Voice in → vocoded out.
    pub fn process_sample(&mut self, voice: f32) -> f32 {
        // Input stage: pre-gain the voice. This is the first line of
        // defense against "everything too quiet" — a hot modulator drives
        // the envelope followers harder, so the vocoder output itself is
        // louder before any post-stage gain.
        let voice = voice * self.input_gain;

        if let Some(est) = self.yin.push(voice) {
            if est.is_voiced() {
                self.last_pitch = est;
            }
        }

        let targets = self.carrier_targets(self.last_pitch.hz);
        let mut carrier_sum = 0.0;
        let mut active = 0;
        for ((osc, porto), target) in self
            .oscs
            .iter_mut()
            .zip(self.portos.iter_mut())
            .zip(targets)
        {
            let smoothed = porto.process(target);
            if smoothed > 0.0 {
                osc.set_frequency(smoothed);
                carrier_sum += osc.tick();
                active += 1;
            }
        }
        if active > 1 {
            // Normalize so chord mode doesn't blow up vs mono.
            carrier_sum /= (active as f32).sqrt();
        }
        carrier_sum *= self.cfg.carrier_level;

        let wet = self.vocoder.process(voice, carrier_sum);
        let mix = self.cfg.dry_wet;
        let mixed = voice * (1.0 - mix) + wet * mix;

        // Output stage: compressor catches peaks from the boosted signal,
        // then output_gain acts as classic post-compressor makeup. Final
        // soft-clamp is belt-and-suspenders for extreme gain settings.
        let compressed = self.compressor.process(mixed);
        let out = compressed * self.output_gain;
        out.clamp(-0.98, 0.98)
    }

    /// Process a buffer in place for convenience.
    pub fn process_buffer(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.process_sample(*s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::TAU;

    #[test]
    fn silence_in_silence_out() {
        let mut av = AutoVocoder::new(48_000.0, AutoVocoderConfig::default());
        let mut buf = vec![0.0f32; 8192];
        av.process_buffer(&mut buf);
        let peak = buf.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        assert!(peak < 1e-3, "silence should stay silent, peak={peak}");
    }

    #[test]
    fn voiced_input_produces_output() {
        let sr = 48_000.0;
        let cfg = AutoVocoderConfig {
            carrier_mode: CarrierMode::Fixed { midi: 48 }, // C3
            ..AutoVocoderConfig::default()
        };
        let mut av = AutoVocoder::new(sr, cfg);
        // Harmonically-rich fake voice: sum of 6 partials at 200 Hz fundamental.
        let mut buf: Vec<f32> = (0..sr as usize * 2)
            .map(|i| {
                let t = i as f32 / sr;
                let mut x = 0.0;
                for h in 1..=6 {
                    x += (TAU * 200.0 * h as f32 * t).sin() / h as f32;
                }
                0.3 * x
            })
            .collect();
        av.process_buffer(&mut buf);
        // Measure RMS over the latter half (past warmup).
        let tail = &buf[buf.len() / 2..];
        let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 0.01, "expected audible output, rms={rms}");
    }
}
