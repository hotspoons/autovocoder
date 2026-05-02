//! Top-level autovocoder: pitch-detect the voice, quantize to scale,
//! synthesize a carrier, drive the vocoder.

use crate::chorus::Chorus;
use crate::crusher::BitCrusher;
use crate::dynamics::{db_to_linear, Compressor};
use crate::osc::{Saw, SubSquare};
use crate::pitch::{PitchAlgorithm, PitchDetector, PitchEstimate};
use crate::saturate::{DriveMode, Saturator};
use crate::scale::{midi_to_hz, quantize_hz_to_scale, Portamento, Scale};
use crate::tremolo::{LfoTarget, Tremolo};
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
    pub pitch_algorithm: PitchAlgorithm,
    // Input stage (pre-vocoder).
    pub input_gain_db: f32, // applied to the voice before everything
    // Output stage.
    pub compressor_enabled: bool,
    pub compressor_threshold_db: f32,
    pub output_gain_db: f32, // applied after the compressor (classic makeup)
    // Chorus — single set of params drives two independent placements.
    // `carrier_chorus_enabled` runs the chorus on the synthesized carrier
    // before it hits the vocoder (animates the timbre of the vocoded voice).
    // `output_chorus_enabled` is classic post-output chorus.
    pub carrier_chorus_enabled: bool,
    pub output_chorus_enabled: bool,
    pub chorus_rate_hz: f32,
    pub chorus_depth: f32, // normalized 0..1
    pub chorus_mix: f32,   // dry/wet 0..1
    // Tremolo — runs on the post-output signal. `shape` morphs sine→square.
    pub tremolo_enabled: bool,
    pub tremolo_rate_hz: f32,
    pub tremolo_depth: f32, // 0..1; full = full chop
    pub tremolo_shape: f32, // 0=sine, 1=near-square
    pub tremolo_target: LfoTarget,
    // Saturation — two placements share mode + drive amount.
    pub pre_drive_enabled: bool,  // saturate the modulator before the vocoder
    pub post_drive_enabled: bool, // saturate the final output
    pub drive_mode: DriveMode,
    pub drive_amount: f32, // 0..1
    // Bit crusher on the output stage (after tremolo, before clamp).
    pub crusher_enabled: bool,
    pub crusher_bits: f32,    // 1..16
    pub crusher_rate: f32,    // 0..1; 1=full sample rate
    // Sub oscillator: square wave at half the carrier root.
    pub sub_enabled: bool,
    pub sub_level: f32, // 0..1, mixed into carrier_sum
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
            // FFT-based YIN by default — same accuracy as classic but ~30×
            // cheaper on the 2k-sample window we use for vocal range.
            pitch_algorithm: PitchAlgorithm::YinFft,
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
            // All effects default to off — preserves the existing sound for
            // existing presets. Users opt in via the LV2 toggles.
            carrier_chorus_enabled: false,
            output_chorus_enabled: false,
            chorus_rate_hz: 0.7,
            chorus_depth: 0.5,
            chorus_mix: 0.5,
            tremolo_enabled: false,
            tremolo_rate_hz: 5.0,
            tremolo_depth: 0.7,
            tremolo_shape: 0.0,
            tremolo_target: LfoTarget::Amplitude,
            pre_drive_enabled: false,
            post_drive_enabled: false,
            drive_mode: DriveMode::Tape,
            drive_amount: 0.5,
            crusher_enabled: false,
            crusher_bits: 16.0,
            crusher_rate: 1.0,
            sub_enabled: false,
            sub_level: 0.5,
        }
    }
}

pub struct AutoVocoder {
    sample_rate: f32,
    cfg: AutoVocoderConfig,
    pitch: PitchDetector,
    last_pitch: PitchEstimate,
    // Up to CARRIER_VOICES oscs. Unused slots stay at 0 Hz so they produce
    // no output. Enough to cover every chord voicing up through 9ths.
    oscs: [Saw; CARRIER_VOICES],
    portos: [Portamento; CARRIER_VOICES],
    vocoder: Vocoder,
    compressor: Compressor,
    input_gain: f32,  // linear
    output_gain: f32, // linear (post-compressor makeup)
    // Hot-path caches. Recomputed only when their inputs change rather than
    // every sample — see `refresh_root()` and `refresh_voicing()`.
    chord_ratios: [f32; CARRIER_VOICES],
    voice_count: usize,
    chord_norm: f32,         // 1 / sqrt(active voices); avoids per-sample sqrt
    cached_root_hz: f32,     // quantized root at the last refresh
    cached_for_pitch_hz: f32, // value of last_pitch.hz used for cached_root_hz
    // Block-processing scratch. Sized by the host's first run() call and
    // grown only if the host bumps its buffer size — both cases are rare
    // outside startup, so we pay an allocation almost never.
    mod_scratch: Vec<f32>,
    car_scratch: Vec<f32>,
    wet_scratch: Vec<f32>,
    /// Pre-saturation copy of the modulator. Holds the clean voice for the
    /// dry tap when pre-drive is engaged (otherwise unused).
    dry_scratch: Vec<f32>,
    // Effects. Two chorus instances so the two placements have un-correlated
    // LFO drift even though they share user-facing rate/depth/mix params.
    carrier_chorus: Chorus,
    output_chorus: Chorus,
    tremolo: Tremolo,
    pre_drive: Saturator,
    post_drive: Saturator,
    crusher: BitCrusher,
    sub_osc: SubSquare,
    // Per-block scratch for the LFO. Filled in the pre-stage of the block
    // path so non-amplitude targets can read precomputed values without
    // double-ticking the LFO oscillator.
    lfo_scratch: Vec<f32>,
}

impl AutoVocoder {
    pub fn new(sample_rate: f32, cfg: AutoVocoderConfig) -> Self {
        let pitch = PitchDetector::new(
            cfg.pitch_algorithm,
            sample_rate,
            cfg.pitch_min_hz,
            cfg.pitch_max_hz,
            256,
        );
        let mut compressor = Compressor::new(sample_rate, cfg.compressor_threshold_db);
        compressor.set_enabled(cfg.compressor_enabled);
        // `[T; N]` from a non-Copy constructor — do it by hand.
        let oscs: [Saw; CARRIER_VOICES] = std::array::from_fn(|_| Saw::new(sample_rate));
        let portos: [Portamento; CARRIER_VOICES] =
            std::array::from_fn(|_| Portamento::new(sample_rate, cfg.portamento_ms));
        let (chord_ratios, voice_count, chord_norm) = compute_voicing(cfg.carrier_mode);
        let mut carrier_chorus = Chorus::new(sample_rate);
        let mut output_chorus = Chorus::new(sample_rate);
        for c in [&mut carrier_chorus, &mut output_chorus] {
            c.set_rate_hz(cfg.chorus_rate_hz);
            c.set_depth(cfg.chorus_depth);
            c.set_mix(cfg.chorus_mix);
        }
        carrier_chorus.set_enabled(cfg.carrier_chorus_enabled);
        output_chorus.set_enabled(cfg.output_chorus_enabled);
        let mut tremolo = Tremolo::new(sample_rate);
        tremolo.set_rate_hz(cfg.tremolo_rate_hz);
        tremolo.set_depth(cfg.tremolo_depth);
        tremolo.set_shape(cfg.tremolo_shape);
        tremolo.set_target(cfg.tremolo_target);
        tremolo.set_enabled(cfg.tremolo_enabled);
        let mut pre_drive = Saturator::new();
        pre_drive.set_mode(cfg.drive_mode);
        pre_drive.set_drive(cfg.drive_amount);
        pre_drive.set_enabled(cfg.pre_drive_enabled);
        let mut post_drive = Saturator::new();
        post_drive.set_mode(cfg.drive_mode);
        post_drive.set_drive(cfg.drive_amount);
        post_drive.set_enabled(cfg.post_drive_enabled);
        let mut crusher = BitCrusher::new();
        crusher.set_bits(cfg.crusher_bits);
        crusher.set_rate(cfg.crusher_rate);
        crusher.set_enabled(cfg.crusher_enabled);
        let sub_osc = SubSquare::new(sample_rate);
        Self {
            sample_rate,
            pitch,
            last_pitch: PitchEstimate::UNVOICED,
            oscs,
            portos,
            vocoder: Vocoder::new(sample_rate, cfg.vocoder),
            compressor,
            input_gain: db_to_linear(cfg.input_gain_db),
            output_gain: db_to_linear(cfg.output_gain_db),
            chord_ratios,
            voice_count,
            chord_norm,
            cached_root_hz: 0.0,
            cached_for_pitch_hz: f32::NAN,
            mod_scratch: Vec::new(),
            car_scratch: Vec::new(),
            wet_scratch: Vec::new(),
            dry_scratch: Vec::new(),
            carrier_chorus,
            output_chorus,
            tremolo,
            pre_drive,
            post_drive,
            crusher,
            sub_osc,
            lfo_scratch: Vec::new(),
            cfg,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn set_scale(&mut self, scale: Scale) {
        self.cfg.scale = scale;
        // Force root recompute on next sample — a different scale can move the
        // quantized root even though the input pitch hasn't changed.
        self.cached_for_pitch_hz = f32::NAN;
    }

    pub fn set_carrier_mode(&mut self, mode: CarrierMode) {
        self.cfg.carrier_mode = mode;
        let (ratios, count, norm) = compute_voicing(mode);
        self.chord_ratios = ratios;
        self.voice_count = count;
        self.chord_norm = norm;
        self.cached_for_pitch_hz = f32::NAN;
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

    // ---- Chorus. Two instances share rate/depth/mix; enables are independent.

    pub fn set_carrier_chorus_enabled(&mut self, on: bool) {
        self.cfg.carrier_chorus_enabled = on;
        self.carrier_chorus.set_enabled(on);
    }

    pub fn set_output_chorus_enabled(&mut self, on: bool) {
        self.cfg.output_chorus_enabled = on;
        self.output_chorus.set_enabled(on);
    }

    pub fn set_chorus_rate_hz(&mut self, hz: f32) {
        self.cfg.chorus_rate_hz = hz;
        self.carrier_chorus.set_rate_hz(hz);
        self.output_chorus.set_rate_hz(hz);
    }

    pub fn set_chorus_depth(&mut self, depth_0_1: f32) {
        self.cfg.chorus_depth = depth_0_1;
        self.carrier_chorus.set_depth(depth_0_1);
        self.output_chorus.set_depth(depth_0_1);
    }

    pub fn set_chorus_mix(&mut self, mix_0_1: f32) {
        self.cfg.chorus_mix = mix_0_1;
        self.carrier_chorus.set_mix(mix_0_1);
        self.output_chorus.set_mix(mix_0_1);
    }

    // ---- Tremolo. Single instance on the post-output signal.

    pub fn set_tremolo_enabled(&mut self, on: bool) {
        self.cfg.tremolo_enabled = on;
        self.tremolo.set_enabled(on);
    }

    pub fn set_tremolo_rate_hz(&mut self, hz: f32) {
        self.cfg.tremolo_rate_hz = hz;
        self.tremolo.set_rate_hz(hz);
    }

    pub fn set_tremolo_depth(&mut self, depth_0_1: f32) {
        self.cfg.tremolo_depth = depth_0_1;
        self.tremolo.set_depth(depth_0_1);
    }

    pub fn set_tremolo_shape(&mut self, shape_0_1: f32) {
        self.cfg.tremolo_shape = shape_0_1;
        self.tremolo.set_shape(shape_0_1);
    }

    pub fn set_tremolo_target(&mut self, target: LfoTarget) {
        self.cfg.tremolo_target = target;
        self.tremolo.set_target(target);
    }

    // ---- Saturation. Two placements share mode + drive amount.

    pub fn set_pre_drive_enabled(&mut self, on: bool) {
        self.cfg.pre_drive_enabled = on;
        self.pre_drive.set_enabled(on);
    }

    pub fn set_post_drive_enabled(&mut self, on: bool) {
        self.cfg.post_drive_enabled = on;
        self.post_drive.set_enabled(on);
    }

    pub fn set_drive_mode(&mut self, mode: DriveMode) {
        self.cfg.drive_mode = mode;
        self.pre_drive.set_mode(mode);
        self.post_drive.set_mode(mode);
    }

    pub fn set_drive_amount(&mut self, amount: f32) {
        self.cfg.drive_amount = amount;
        self.pre_drive.set_drive(amount);
        self.post_drive.set_drive(amount);
    }

    // ---- Bit crusher.

    pub fn set_crusher_enabled(&mut self, on: bool) {
        self.cfg.crusher_enabled = on;
        self.crusher.set_enabled(on);
    }

    pub fn set_crusher_bits(&mut self, bits: f32) {
        self.cfg.crusher_bits = bits;
        self.crusher.set_bits(bits);
    }

    pub fn set_crusher_rate(&mut self, rate_0_1: f32) {
        self.cfg.crusher_rate = rate_0_1;
        self.crusher.set_rate(rate_0_1);
    }

    // ---- Sub oscillator.

    pub fn set_sub_enabled(&mut self, on: bool) {
        self.cfg.sub_enabled = on;
        if !on {
            self.sub_osc.reset_phase();
        }
    }

    pub fn set_sub_level(&mut self, level: f32) {
        self.cfg.sub_level = level.clamp(0.0, 1.0);
    }

    pub fn set_pitch_algorithm(&mut self, algo: PitchAlgorithm) {
        if self.pitch.algorithm() == algo {
            return;
        }
        self.cfg.pitch_algorithm = algo;
        // Rebuild — each variant maintains its own ring buffer / FFT plan.
        // Last pitch resets so we don't smear stale state across the swap.
        self.pitch = PitchDetector::new(
            algo,
            self.sample_rate,
            self.cfg.pitch_min_hz,
            self.cfg.pitch_max_hz,
            256,
        );
        self.last_pitch = PitchEstimate::UNVOICED;
        self.cached_for_pitch_hz = f32::NAN;
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
        self.carrier_chorus.reset();
        self.output_chorus.reset();
        self.tremolo.reset();
        self.crusher.reset();
        self.sub_osc.reset_phase();
    }

    /// Refresh the cached quantized/fixed root if the voice pitch has changed
    /// since we last computed it. Called once per sample — most samples hit
    /// the early-return because YIN only updates the pitch on hop boundaries.
    #[inline]
    fn refresh_root(&mut self) {
        if self.cached_for_pitch_hz == self.last_pitch.hz {
            return;
        }
        self.cached_root_hz = match self.cfg.carrier_mode {
            CarrierMode::Mono | CarrierMode::Chord(_) => {
                quantize_hz_to_scale(self.last_pitch.hz, self.cfg.scale)
            }
            CarrierMode::Fixed { midi } | CarrierMode::FixedChord { midi, .. } => {
                midi_to_hz(midi as f32)
            }
        };
        self.cached_for_pitch_hz = self.last_pitch.hz;
    }

    /// Process one sample. Voice in → vocoded out.
    ///
    /// Signal flow (effects pass through when disabled):
    ///   in × input_gain → YIN push (uses clean voice for pitch)
    ///                  → pre-drive (saturates the modulator only)
    ///   carrier synth → +sub_osc → ×carrier_level → carrier_chorus → vocoder
    ///   dry voice + wet → compressor → output_gain → output_chorus
    ///                  → amp tremolo → post-drive → bit crusher → clamp
    /// The mod LFO ticks once per sample; whichever target is selected
    /// reads from that single tick.
    pub fn process_sample(&mut self, voice: f32) -> f32 {
        // Input stage. Pre-gain feeds YIN with a hot signal, which makes
        // pitch detection more reliable for quiet sources.
        let voice = voice * self.input_gain;

        if let Some(est) = self.pitch.push(voice) {
            if est.is_voiced() {
                self.last_pitch = est;
            }
        }

        self.refresh_root();
        // One LFO tick. Cheap when disabled (returns 0.0, no phase advance).
        let lfo = self.tremolo.tick_lfo();
        let target = self.tremolo.target();

        let pitch_mult = if target == LfoTarget::Pitch {
            self.tremolo.pitch_mult(lfo)
        } else {
            1.0
        };
        let level_mult = if target == LfoTarget::CarrierLevel {
            self.tremolo.carrier_level_mult(lfo)
        } else {
            1.0
        };

        let root = self.cached_root_hz * pitch_mult;
        let mut carrier_sum = 0.0;
        if root > 0.0 {
            for ((osc, porto), &ratio) in self
                .oscs
                .iter_mut()
                .zip(self.portos.iter_mut())
                .zip(self.chord_ratios.iter())
            {
                if ratio == 0.0 {
                    continue;
                }
                let smoothed = porto.process(root * ratio);
                osc.set_frequency(smoothed);
                carrier_sum += osc.tick();
            }
            carrier_sum *= self.chord_norm;
            // Sub-square at half the (vibrato-modulated) root. Mixed in
            // *after* chord normalization so its level reads consistently
            // regardless of how many chord voices are active.
            if self.cfg.sub_enabled {
                self.sub_osc.set_frequency(root * 0.5);
                carrier_sum += self.sub_osc.tick() * self.cfg.sub_level;
            } else {
                // Keep phase coherent for the next time it's enabled.
                self.sub_osc.set_frequency(0.0);
                let _ = self.sub_osc.tick();
            }
        }
        carrier_sum *= self.cfg.carrier_level * level_mult;
        let carrier_sum = self.carrier_chorus.process_sample(carrier_sum);

        // Modulator path: optional pre-vocoder saturation. Coloring the
        // modulator changes the envelope followers' input, which shifts
        // the vocoder character without affecting pitch detection (already
        // done above on the clean signal).
        let modulator = self.pre_drive.process_sample(voice);
        let wet = self.vocoder.process(modulator, carrier_sum);

        // Dry/wet mix. Dry tap uses the *clean* voice (post-input-gain,
        // pre-saturation) so the dry side stays untouched.
        let mix = if target == LfoTarget::DryWet {
            (self.cfg.dry_wet + self.tremolo.drywet_offset(lfo)).clamp(0.0, 1.0)
        } else {
            self.cfg.dry_wet
        };
        let mixed = voice * (1.0 - mix) + wet * mix;

        // Output stage. compressor → makeup → output_chorus → amp tremolo
        // → post-drive (overdrive after modulation, like a pedalboard) →
        // bit crusher → soft clamp.
        let compressed = self.compressor.process(mixed);
        let gained = compressed * self.output_gain;
        let chorused = self.output_chorus.process_sample(gained);
        let tremmed = if target == LfoTarget::Amplitude && self.tremolo.enabled() {
            chorused * self.tremolo.amp_gain(lfo)
        } else {
            chorused
        };
        let driven = self.post_drive.process_sample(tremmed);
        let crushed = self.crusher.process_sample(driven);
        crushed.clamp(-0.98, 0.98)
    }

    /// Process a buffer in place for convenience.
    pub fn process_buffer(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.process_sample(*s);
        }
    }

    /// Block-based processing. The path real-time hosts (LV2, JACK) take.
    ///
    /// Splits the work into three contiguous passes over the block:
    ///   1. pre-stage — input gain, YIN push, carrier oscillators
    ///   2. vocoder — runs one band at a time across the whole block, so
    ///      each band's biquad/env state stays in registers rather than
    ///      getting reloaded per sample × 16 bands
    ///   3. post-stage — dry/wet, compressor, output gain, soft clamp
    ///
    /// Equivalent to a per-sample loop over `process_sample` but materially
    /// less cache thrash and friendlier to autovectorization.
    pub fn process_block(&mut self, input: &[f32], output: &mut [f32]) {
        let n = input.len();
        debug_assert_eq!(output.len(), n);
        if n == 0 {
            return;
        }
        if self.mod_scratch.len() < n {
            self.mod_scratch.resize(n, 0.0);
            self.car_scratch.resize(n, 0.0);
            self.wet_scratch.resize(n, 0.0);
            self.dry_scratch.resize(n, 0.0);
            self.lfo_scratch.resize(n, 0.0);
        }

        let target = self.tremolo.target();

        // ---- Pass 1: pre-stage. Per-sample because YIN + portamento +
        // oscillator phase + the LFO are inherently sequential. Borrow
        // scratch in a tight scope so subsequent passes can re-borrow `self`.
        {
            let mod_buf = &mut self.mod_scratch[..n];
            let car_buf = &mut self.car_scratch[..n];
            let lfo_buf = &mut self.lfo_scratch[..n];
            let input_gain = self.input_gain;
            let base_carrier_level = self.cfg.carrier_level;
            let sub_enabled = self.cfg.sub_enabled;
            let sub_level = self.cfg.sub_level;
            for i in 0..n {
                let v = input[i] * input_gain;
                if let Some(est) = self.pitch.push(v) {
                    if est.is_voiced() {
                        self.last_pitch = est;
                    }
                }
                // Inline `refresh_root` — borrow checker can't see through
                // the slices above to call the method.
                if self.cached_for_pitch_hz != self.last_pitch.hz {
                    self.cached_root_hz = match self.cfg.carrier_mode {
                        CarrierMode::Mono | CarrierMode::Chord(_) => {
                            quantize_hz_to_scale(self.last_pitch.hz, self.cfg.scale)
                        }
                        CarrierMode::Fixed { midi }
                        | CarrierMode::FixedChord { midi, .. } => midi_to_hz(midi as f32),
                    };
                    self.cached_for_pitch_hz = self.last_pitch.hz;
                }

                // Tick the LFO once per sample. Result reused below for
                // pitch / carrier-level targets and stashed for post-stage
                // dry/wet and amplitude targets.
                let lfo = self.tremolo.tick_lfo();
                lfo_buf[i] = lfo;
                let pitch_mult = if target == LfoTarget::Pitch {
                    self.tremolo.pitch_mult(lfo)
                } else {
                    1.0
                };
                let level_mult = if target == LfoTarget::CarrierLevel {
                    self.tremolo.carrier_level_mult(lfo)
                } else {
                    1.0
                };

                let root = self.cached_root_hz * pitch_mult;
                let mut carrier = 0.0;
                if root > 0.0 {
                    for ((osc, porto), &ratio) in self
                        .oscs
                        .iter_mut()
                        .zip(self.portos.iter_mut())
                        .zip(self.chord_ratios.iter())
                    {
                        if ratio == 0.0 {
                            continue;
                        }
                        let smoothed = porto.process(root * ratio);
                        osc.set_frequency(smoothed);
                        carrier += osc.tick();
                    }
                    carrier *= self.chord_norm;
                    if sub_enabled {
                        self.sub_osc.set_frequency(root * 0.5);
                        carrier += self.sub_osc.tick() * sub_level;
                    } else {
                        self.sub_osc.set_frequency(0.0);
                        let _ = self.sub_osc.tick();
                    }
                }
                mod_buf[i] = v;
                car_buf[i] = carrier * base_carrier_level * level_mult;
            }
        }

        // ---- Pass 2a: carrier chorus on the carrier buffer.
        self.carrier_chorus.process_block(&mut self.car_scratch[..n]);

        // ---- Pass 2b: pre-vocoder saturation on the modulator. The dry
        // tap downstream needs the clean signal, so when pre-drive is on
        // we keep a copy in `dry_scratch` and saturate `mod_scratch` in
        // place. When pre-drive is off, both paths see the same clean
        // signal and we skip the copy entirely.
        let dry_is_separate = self.pre_drive.enabled();
        if dry_is_separate {
            self.dry_scratch[..n].copy_from_slice(&self.mod_scratch[..n]);
            self.pre_drive.process_block(&mut self.mod_scratch[..n]);
        }

        // ---- Pass 2c: vocoder, band-major.
        self.vocoder.process_block(
            &self.mod_scratch[..n],
            &self.car_scratch[..n],
            &mut self.wet_scratch[..n],
        );

        // ---- Pass 3a: dry/wet mix. Pull dry from dry_scratch (clean) when
        // pre-drive ran, otherwise from mod_scratch.
        let mix_base = self.cfg.dry_wet;
        let dry_buf: &[f32] = if dry_is_separate {
            &self.dry_scratch[..n]
        } else {
            &self.mod_scratch[..n]
        };
        let wet_buf = &self.wet_scratch[..n];
        let lfo_buf = &self.lfo_scratch[..n];
        let trem = &self.tremolo;
        for i in 0..n {
            let m = if target == LfoTarget::DryWet {
                (mix_base + trem.drywet_offset(lfo_buf[i])).clamp(0.0, 1.0)
            } else {
                mix_base
            };
            output[i] = dry_buf[i] * (1.0 - m) + wet_buf[i] * m;
        }

        // ---- Pass 3: compressor + makeup gain (per-sample, sequential).
        let output_gain = self.output_gain;
        for s in output[..n].iter_mut() {
            *s = self.compressor.process(*s) * output_gain;
        }

        // ---- Pass 4: output chorus, then amplitude tremolo (if that's the
        // target), then post-drive, then crusher, then soft clamp.
        self.output_chorus.process_block(&mut output[..n]);
        if target == LfoTarget::Amplitude && self.tremolo.enabled() {
            for (s, &lfo) in output[..n].iter_mut().zip(self.lfo_scratch[..n].iter()) {
                *s *= self.tremolo.amp_gain(lfo);
            }
        }
        self.post_drive.process_block(&mut output[..n]);
        self.crusher.process_block(&mut output[..n]);
        for s in output[..n].iter_mut() {
            *s = s.clamp(-0.98, 0.98);
        }
    }
}


/// Voicing → (per-voice frequency ratios, voice count, output normalization).
/// Lifted out of the per-sample path because chord intervals are constants —
/// the old code did `2f32.powf(semis as f32 / 12.0)` per voice per sample.
fn compute_voicing(mode: CarrierMode) -> ([f32; CARRIER_VOICES], usize, f32) {
    let mut ratios = [0.0f32; CARRIER_VOICES];
    let voicing = match mode {
        CarrierMode::Mono | CarrierMode::Fixed { .. } => None,
        CarrierMode::Chord(v) | CarrierMode::FixedChord { voicing: v, .. } => Some(v),
    };
    let intervals: &[i8] = match voicing {
        None => &[0],
        Some(v) => v.intervals(),
    };
    let count = intervals.len().min(CARRIER_VOICES);
    for (i, &semis) in intervals.iter().take(CARRIER_VOICES).enumerate() {
        ratios[i] = 2f32.powf(semis as f32 / 12.0);
    }
    // Mirrors the old `1 / sqrt(active)` normalization (only kicks in for chords).
    let norm = if count > 1 {
        1.0 / (count as f32).sqrt()
    } else {
        1.0
    };
    (ratios, count, norm)
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
    fn block_matches_per_sample() {
        // process_block must produce bit-identical output to a per-sample
        // loop — same DSP, just rearranged. If they ever drift, something
        // about the block path is doing different math.
        let sr = 48_000.0;
        let cfg = AutoVocoderConfig {
            carrier_mode: CarrierMode::Fixed { midi: 48 },
            ..AutoVocoderConfig::default()
        };
        let mut av_a = AutoVocoder::new(sr, cfg);
        let mut av_b = AutoVocoder::new(sr, cfg);
        // Quasi-realistic input: a couple of partials.
        let n = 4096;
        let input: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                0.3 * ((TAU * 220.0 * t).sin() + 0.5 * (TAU * 440.0 * t).sin())
            })
            .collect();
        let mut buf_a = input.clone();
        av_a.process_buffer(&mut buf_a);
        let mut buf_b = vec![0.0f32; n];
        // Run two blocks of unequal size to exercise the resize path.
        av_b.process_block(&input[..1024], &mut buf_b[..1024]);
        av_b.process_block(&input[1024..], &mut buf_b[1024..]);
        for (i, (a, b)) in buf_a.iter().zip(buf_b.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "drift at sample {i}: per-sample={a}, block={b}"
            );
        }
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
