//! LV2 plugin wrapper around `autovocoder-dsp`.
//!
//! This crate intentionally avoids the `lv2-rs` bindings family to keep the
//! dep tree tiny and the ABI transparent. The LV2 C ABI is simple and stable;
//! we define only the types we actually use below.
//!
//! Ports (see lv2/autovocoder.ttl for matching symbol names and ranges):
//!   0  AudioInput
//!   1  AudioOutput
//!   2  Mode          (int: 0=mono, 1=chord, 2=fixed, 3=fixed_chord)
//!   3  FixedNote     (int MIDI 0..127; used when Mode==fixed or fixed_chord)
//!   4  Scale         (int: 0=chromatic, 1=major, 2=minor)
//!   5  ScaleRoot     (int pitch class 0..11)
//!   6  Mix           (float 0..1, dry/wet)
//!   7  Portamento    (float 1..500 ms)
//!   8  CarrierLevel  (float 0..2)
//!   9  InputGain     (float -20..+60 dB; pre-vocoder)
//!  10  OutputGain    (float -20..+60 dB; post-compressor makeup)
//!  11  CompOn        (int 0/1; enable the built-in compressor)
//!  12  CompThreshold (float -40..0 dB; compressor threshold)
//!  13  ChordType     (int 0..14; voicing for Chord / FixedChord modes)
//!  14  PitchAlgo     (int 0=YinClassic, 1=YinFft, 2=FftPeak)
//!  15  CarrierChorusOn (int 0/1; chorus on the synthesized carrier pre-vocoder)
//!  16  OutputChorusOn  (int 0/1; chorus on the post-output signal)
//!  17  ChorusRate    (float 0.05..5 Hz; shared between both chorus instances)
//!  18  ChorusDepth   (float 0..1)
//!  19  ChorusMix     (float 0..1)
//!  20  TremOn        (int 0/1; output amplitude modulator)
//!  21  TremRate      (float 0.1..20 Hz)
//!  22  TremDepth     (float 0..1)
//!  23  TremShape     (float 0..1; sine ↔ near-square morph)
//!  24  TremTarget    (int 0=Amp, 1=Pitch, 2=DryWet, 3=CarrierLevel)
//!  25  PreDriveOn    (int 0/1; saturate the modulator before vocoding)
//!  26  PostDriveOn   (int 0/1; saturate the final output)
//!  27  DriveMode     (int 0=Tube, 1=Tape, 2=Fuzz)
//!  28  DriveAmount   (float 0..1)
//!  29  CrusherOn     (int 0/1)
//!  30  CrusherBits   (int 1..16; lower = more crush)
//!  31  CrusherRate   (float 0..1; 1=full sample rate)
//!  32  SubOn         (int 0/1; square wave at half the carrier root)
//!  33  SubLevel      (float 0..1)

#![allow(non_camel_case_types)]
#![allow(clippy::missing_safety_doc)]

use autovocoder_dsp::{
    AutoVocoder, AutoVocoderConfig, CarrierMode, ChordVoicing, DriveMode, LfoTarget,
    PitchAlgorithm, Scale,
};
use std::ffi::{c_char, c_void, CStr};
use std::ptr;

// --- Minimal LV2 C ABI ------------------------------------------------------

#[repr(C)]
pub struct LV2_Descriptor {
    pub uri: *const c_char,
    pub instantiate: Option<
        unsafe extern "C" fn(
            descriptor: *const LV2_Descriptor,
            sample_rate: f64,
            bundle_path: *const c_char,
            features: *const *const LV2_Feature,
        ) -> LV2_Handle,
    >,
    pub connect_port:
        Option<unsafe extern "C" fn(instance: LV2_Handle, port: u32, data: *mut c_void)>,
    pub activate: Option<unsafe extern "C" fn(instance: LV2_Handle)>,
    pub run: Option<unsafe extern "C" fn(instance: LV2_Handle, n_samples: u32)>,
    pub deactivate: Option<unsafe extern "C" fn(instance: LV2_Handle)>,
    pub cleanup: Option<unsafe extern "C" fn(instance: LV2_Handle)>,
    pub extension_data: Option<unsafe extern "C" fn(uri: *const c_char) -> *const c_void>,
}

pub type LV2_Handle = *mut c_void;

#[repr(C)]
pub struct LV2_Feature {
    pub uri: *const c_char,
    pub data: *mut c_void,
}

// --- Plugin --------------------------------------------------------------

const PLUGIN_URI: &[u8] = b"https://github.com/hotspoons/autovocoder\0";

// Port indices — MUST match autovocoder.ttl.
const PORT_IN: u32 = 0;
const PORT_OUT: u32 = 1;
const PORT_MODE: u32 = 2;
const PORT_FIXED_NOTE: u32 = 3;
const PORT_SCALE: u32 = 4;
const PORT_SCALE_ROOT: u32 = 5;
const PORT_MIX: u32 = 6;
const PORT_PORTAMENTO: u32 = 7;
const PORT_CARRIER_LEVEL: u32 = 8;
const PORT_INPUT_GAIN: u32 = 9;
const PORT_OUTPUT_GAIN: u32 = 10;
const PORT_COMP_ON: u32 = 11;
const PORT_COMP_THRESHOLD: u32 = 12;
const PORT_CHORD_TYPE: u32 = 13;
const PORT_PITCH_ALGO: u32 = 14;
const PORT_CARRIER_CHORUS_ON: u32 = 15;
const PORT_OUTPUT_CHORUS_ON: u32 = 16;
const PORT_CHORUS_RATE: u32 = 17;
const PORT_CHORUS_DEPTH: u32 = 18;
const PORT_CHORUS_MIX: u32 = 19;
const PORT_TREM_ON: u32 = 20;
const PORT_TREM_RATE: u32 = 21;
const PORT_TREM_DEPTH: u32 = 22;
const PORT_TREM_SHAPE: u32 = 23;
const PORT_TREM_TARGET: u32 = 24;
const PORT_PRE_DRIVE_ON: u32 = 25;
const PORT_POST_DRIVE_ON: u32 = 26;
const PORT_DRIVE_MODE: u32 = 27;
const PORT_DRIVE_AMOUNT: u32 = 28;
const PORT_CRUSHER_ON: u32 = 29;
const PORT_CRUSHER_BITS: u32 = 30;
const PORT_CRUSHER_RATE: u32 = 31;
const PORT_SUB_ON: u32 = 32;
const PORT_SUB_LEVEL: u32 = 33;

struct Plugin {
    av: AutoVocoder,

    // Audio ports — pointers owned by the host, updated via connect_port.
    in_buf: *const f32,
    out_buf: *mut f32,

    // Control ports — f32 pointers that the host writes. Read each run().
    mode: *const f32,
    fixed_note: *const f32,
    scale_kind: *const f32,
    scale_root: *const f32,
    mix: *const f32,
    portamento: *const f32,
    carrier_level: *const f32,
    input_gain: *const f32,
    output_gain: *const f32,
    comp_on: *const f32,
    comp_threshold: *const f32,
    chord_type: *const f32,
    pitch_algo: *const f32,
    carrier_chorus_on: *const f32,
    output_chorus_on: *const f32,
    chorus_rate: *const f32,
    chorus_depth: *const f32,
    chorus_mix: *const f32,
    trem_on: *const f32,
    trem_rate: *const f32,
    trem_depth: *const f32,
    trem_shape: *const f32,
    trem_target: *const f32,
    pre_drive_on: *const f32,
    post_drive_on: *const f32,
    drive_mode: *const f32,
    drive_amount: *const f32,
    crusher_on: *const f32,
    crusher_bits: *const f32,
    crusher_rate: *const f32,
    sub_on: *const f32,
    sub_level: *const f32,

    // Last-seen values, so we only push into DSP on change (cheap RT-safe).
    last_mode: i32,
    last_fixed_note: i32,
    last_scale_kind: i32,
    last_scale_root: i32,
    last_mix: f32,
    last_portamento: f32,
    last_carrier_level: f32,
    last_input_gain: f32,
    last_output_gain: f32,
    last_comp_on: i32,
    last_comp_threshold: f32,
    last_chord_type: i32,
    last_pitch_algo: i32,
    last_carrier_chorus_on: i32,
    last_output_chorus_on: i32,
    last_chorus_rate: f32,
    last_chorus_depth: f32,
    last_chorus_mix: f32,
    last_trem_on: i32,
    last_trem_rate: f32,
    last_trem_depth: f32,
    last_trem_shape: f32,
    last_trem_target: i32,
    last_pre_drive_on: i32,
    last_post_drive_on: i32,
    last_drive_mode: i32,
    last_drive_amount: f32,
    last_crusher_on: i32,
    last_crusher_bits: i32,
    last_crusher_rate: f32,
    last_sub_on: i32,
    last_sub_level: f32,
}

unsafe extern "C" fn instantiate(
    _desc: *const LV2_Descriptor,
    sample_rate: f64,
    _bundle_path: *const c_char,
    _features: *const *const LV2_Feature,
) -> LV2_Handle {
    let sr = sample_rate as f32;
    let av = AutoVocoder::new(sr, AutoVocoderConfig::default());
    let p = Box::new(Plugin {
        av,
        in_buf: ptr::null(),
        out_buf: ptr::null_mut(),
        mode: ptr::null(),
        fixed_note: ptr::null(),
        scale_kind: ptr::null(),
        scale_root: ptr::null(),
        mix: ptr::null(),
        portamento: ptr::null(),
        carrier_level: ptr::null(),
        input_gain: ptr::null(),
        output_gain: ptr::null(),
        comp_on: ptr::null(),
        comp_threshold: ptr::null(),
        chord_type: ptr::null(),
        pitch_algo: ptr::null(),
        carrier_chorus_on: ptr::null(),
        output_chorus_on: ptr::null(),
        chorus_rate: ptr::null(),
        chorus_depth: ptr::null(),
        chorus_mix: ptr::null(),
        trem_on: ptr::null(),
        trem_rate: ptr::null(),
        trem_depth: ptr::null(),
        trem_shape: ptr::null(),
        trem_target: ptr::null(),
        pre_drive_on: ptr::null(),
        post_drive_on: ptr::null(),
        drive_mode: ptr::null(),
        drive_amount: ptr::null(),
        crusher_on: ptr::null(),
        crusher_bits: ptr::null(),
        crusher_rate: ptr::null(),
        sub_on: ptr::null(),
        sub_level: ptr::null(),
        last_mode: -1,
        last_fixed_note: -1,
        last_scale_kind: -1,
        last_scale_root: -1,
        last_mix: f32::INFINITY,
        last_portamento: f32::INFINITY,
        last_carrier_level: f32::INFINITY,
        last_input_gain: f32::INFINITY,
        last_output_gain: f32::INFINITY,
        last_comp_on: -1,
        last_comp_threshold: f32::INFINITY,
        last_chord_type: -1,
        last_pitch_algo: -1,
        last_carrier_chorus_on: -1,
        last_output_chorus_on: -1,
        last_chorus_rate: f32::INFINITY,
        last_chorus_depth: f32::INFINITY,
        last_chorus_mix: f32::INFINITY,
        last_trem_on: -1,
        last_trem_rate: f32::INFINITY,
        last_trem_depth: f32::INFINITY,
        last_trem_shape: f32::INFINITY,
        last_trem_target: -1,
        last_pre_drive_on: -1,
        last_post_drive_on: -1,
        last_drive_mode: -1,
        last_drive_amount: f32::INFINITY,
        last_crusher_on: -1,
        last_crusher_bits: -1,
        last_crusher_rate: f32::INFINITY,
        last_sub_on: -1,
        last_sub_level: f32::INFINITY,
    });
    Box::into_raw(p) as LV2_Handle
}

unsafe extern "C" fn connect_port(instance: LV2_Handle, port: u32, data: *mut c_void) {
    let p = &mut *(instance as *mut Plugin);
    match port {
        PORT_IN => p.in_buf = data as *const f32,
        PORT_OUT => p.out_buf = data as *mut f32,
        PORT_MODE => p.mode = data as *const f32,
        PORT_FIXED_NOTE => p.fixed_note = data as *const f32,
        PORT_SCALE => p.scale_kind = data as *const f32,
        PORT_SCALE_ROOT => p.scale_root = data as *const f32,
        PORT_MIX => p.mix = data as *const f32,
        PORT_PORTAMENTO => p.portamento = data as *const f32,
        PORT_CARRIER_LEVEL => p.carrier_level = data as *const f32,
        PORT_INPUT_GAIN => p.input_gain = data as *const f32,
        PORT_OUTPUT_GAIN => p.output_gain = data as *const f32,
        PORT_COMP_ON => p.comp_on = data as *const f32,
        PORT_COMP_THRESHOLD => p.comp_threshold = data as *const f32,
        PORT_CHORD_TYPE => p.chord_type = data as *const f32,
        PORT_PITCH_ALGO => p.pitch_algo = data as *const f32,
        PORT_CARRIER_CHORUS_ON => p.carrier_chorus_on = data as *const f32,
        PORT_OUTPUT_CHORUS_ON => p.output_chorus_on = data as *const f32,
        PORT_CHORUS_RATE => p.chorus_rate = data as *const f32,
        PORT_CHORUS_DEPTH => p.chorus_depth = data as *const f32,
        PORT_CHORUS_MIX => p.chorus_mix = data as *const f32,
        PORT_TREM_ON => p.trem_on = data as *const f32,
        PORT_TREM_RATE => p.trem_rate = data as *const f32,
        PORT_TREM_DEPTH => p.trem_depth = data as *const f32,
        PORT_TREM_SHAPE => p.trem_shape = data as *const f32,
        PORT_TREM_TARGET => p.trem_target = data as *const f32,
        PORT_PRE_DRIVE_ON => p.pre_drive_on = data as *const f32,
        PORT_POST_DRIVE_ON => p.post_drive_on = data as *const f32,
        PORT_DRIVE_MODE => p.drive_mode = data as *const f32,
        PORT_DRIVE_AMOUNT => p.drive_amount = data as *const f32,
        PORT_CRUSHER_ON => p.crusher_on = data as *const f32,
        PORT_CRUSHER_BITS => p.crusher_bits = data as *const f32,
        PORT_CRUSHER_RATE => p.crusher_rate = data as *const f32,
        PORT_SUB_ON => p.sub_on = data as *const f32,
        PORT_SUB_LEVEL => p.sub_level = data as *const f32,
        _ => {}
    }
}

unsafe extern "C" fn activate(instance: LV2_Handle) {
    let p = &mut *(instance as *mut Plugin);
    p.av.reset();
}

unsafe extern "C" fn run(instance: LV2_Handle, n_samples: u32) {
    let p = &mut *(instance as *mut Plugin);

    // Apply any control-port changes since last run.
    apply_controls(p);

    if p.in_buf.is_null() || p.out_buf.is_null() {
        return;
    }
    let n = n_samples as usize;
    let input = std::slice::from_raw_parts(p.in_buf, n);
    let output = std::slice::from_raw_parts_mut(p.out_buf, n);
    p.av.process_block(input, output);
}

unsafe fn apply_controls(p: &mut Plugin) {
    // Each control port is an f32 written by the host. Null = host didn't
    // connect it; fall back to current DSP defaults.
    let mode_i = read_int(p.mode, p.last_mode);
    let fixed_i = read_int(p.fixed_note, p.last_fixed_note.max(0));
    let scale_i = read_int(p.scale_kind, p.last_scale_kind);
    let root_i = read_int(p.scale_root, p.last_scale_root.max(0));
    let chord_i = read_int(p.chord_type, p.last_chord_type.max(0));

    // Any of mode / fixed-note / chord-type changing means rebuild CarrierMode.
    if mode_i != p.last_mode || fixed_i != p.last_fixed_note || chord_i != p.last_chord_type {
        let voicing = ChordVoicing::from_int(chord_i);
        let midi = fixed_i.clamp(0, 127) as u8;
        p.av.set_carrier_mode(match mode_i {
            1 => CarrierMode::Chord(voicing),
            2 => CarrierMode::Fixed { midi },
            3 => CarrierMode::FixedChord { midi, voicing },
            _ => CarrierMode::Mono,
        });
        p.last_mode = mode_i;
        p.last_fixed_note = fixed_i;
        p.last_chord_type = chord_i;
    }

    if scale_i != p.last_scale_kind || root_i != p.last_scale_root {
        let root_pc = root_i.rem_euclid(12) as u8;
        p.av.set_scale(Scale::from_int(scale_i, root_pc));
        p.last_scale_kind = scale_i;
        p.last_scale_root = root_i;
    }

    if let Some(m) = read_float(p.mix) {
        if (m - p.last_mix).abs() > 1e-4 {
            p.av.set_dry_wet(m);
            p.last_mix = m;
        }
    }
    if let Some(ms) = read_float(p.portamento) {
        if (ms - p.last_portamento).abs() > 1e-3 {
            p.av.set_portamento_ms(ms);
            p.last_portamento = ms;
        }
    }
    if let Some(l) = read_float(p.carrier_level) {
        if (l - p.last_carrier_level).abs() > 1e-4 {
            p.av.set_carrier_level(l);
            p.last_carrier_level = l;
        }
    }
    if let Some(g) = read_float(p.input_gain) {
        if (g - p.last_input_gain).abs() > 1e-3 {
            p.av.set_input_gain_db(g);
            p.last_input_gain = g;
        }
    }
    if let Some(g) = read_float(p.output_gain) {
        if (g - p.last_output_gain).abs() > 1e-3 {
            p.av.set_output_gain_db(g);
            p.last_output_gain = g;
        }
    }
    let comp_on_i = read_int(p.comp_on, p.last_comp_on.max(0));
    if comp_on_i != p.last_comp_on {
        p.av.set_compressor_enabled(comp_on_i != 0);
        p.last_comp_on = comp_on_i;
    }
    if let Some(t) = read_float(p.comp_threshold) {
        if (t - p.last_comp_threshold).abs() > 1e-3 {
            p.av.set_compressor_threshold_db(t);
            p.last_comp_threshold = t;
        }
    }
    let algo_i = read_int(p.pitch_algo, p.last_pitch_algo.max(0));
    if algo_i != p.last_pitch_algo {
        p.av.set_pitch_algorithm(PitchAlgorithm::from_int(algo_i));
        p.last_pitch_algo = algo_i;
    }

    // Chorus.
    let cc_on = read_int(p.carrier_chorus_on, p.last_carrier_chorus_on.max(0));
    if cc_on != p.last_carrier_chorus_on {
        p.av.set_carrier_chorus_enabled(cc_on != 0);
        p.last_carrier_chorus_on = cc_on;
    }
    let oc_on = read_int(p.output_chorus_on, p.last_output_chorus_on.max(0));
    if oc_on != p.last_output_chorus_on {
        p.av.set_output_chorus_enabled(oc_on != 0);
        p.last_output_chorus_on = oc_on;
    }
    if let Some(v) = read_float(p.chorus_rate) {
        if (v - p.last_chorus_rate).abs() > 1e-3 {
            p.av.set_chorus_rate_hz(v);
            p.last_chorus_rate = v;
        }
    }
    if let Some(v) = read_float(p.chorus_depth) {
        if (v - p.last_chorus_depth).abs() > 1e-4 {
            p.av.set_chorus_depth(v);
            p.last_chorus_depth = v;
        }
    }
    if let Some(v) = read_float(p.chorus_mix) {
        if (v - p.last_chorus_mix).abs() > 1e-4 {
            p.av.set_chorus_mix(v);
            p.last_chorus_mix = v;
        }
    }

    // Tremolo.
    let trem_on = read_int(p.trem_on, p.last_trem_on.max(0));
    if trem_on != p.last_trem_on {
        p.av.set_tremolo_enabled(trem_on != 0);
        p.last_trem_on = trem_on;
    }
    if let Some(v) = read_float(p.trem_rate) {
        if (v - p.last_trem_rate).abs() > 1e-3 {
            p.av.set_tremolo_rate_hz(v);
            p.last_trem_rate = v;
        }
    }
    if let Some(v) = read_float(p.trem_depth) {
        if (v - p.last_trem_depth).abs() > 1e-4 {
            p.av.set_tremolo_depth(v);
            p.last_trem_depth = v;
        }
    }
    if let Some(v) = read_float(p.trem_shape) {
        if (v - p.last_trem_shape).abs() > 1e-4 {
            p.av.set_tremolo_shape(v);
            p.last_trem_shape = v;
        }
    }
    let trem_target_i = read_int(p.trem_target, p.last_trem_target.max(0));
    if trem_target_i != p.last_trem_target {
        p.av.set_tremolo_target(LfoTarget::from_int(trem_target_i));
        p.last_trem_target = trem_target_i;
    }

    // Saturation.
    let pre_on = read_int(p.pre_drive_on, p.last_pre_drive_on.max(0));
    if pre_on != p.last_pre_drive_on {
        p.av.set_pre_drive_enabled(pre_on != 0);
        p.last_pre_drive_on = pre_on;
    }
    let post_on = read_int(p.post_drive_on, p.last_post_drive_on.max(0));
    if post_on != p.last_post_drive_on {
        p.av.set_post_drive_enabled(post_on != 0);
        p.last_post_drive_on = post_on;
    }
    let drive_mode_i = read_int(p.drive_mode, p.last_drive_mode.max(0));
    if drive_mode_i != p.last_drive_mode {
        p.av.set_drive_mode(DriveMode::from_int(drive_mode_i));
        p.last_drive_mode = drive_mode_i;
    }
    if let Some(v) = read_float(p.drive_amount) {
        if (v - p.last_drive_amount).abs() > 1e-4 {
            p.av.set_drive_amount(v);
            p.last_drive_amount = v;
        }
    }

    // Bit crusher.
    let crusher_on_i = read_int(p.crusher_on, p.last_crusher_on.max(0));
    if crusher_on_i != p.last_crusher_on {
        p.av.set_crusher_enabled(crusher_on_i != 0);
        p.last_crusher_on = crusher_on_i;
    }
    let crusher_bits_i = read_int(p.crusher_bits, p.last_crusher_bits.max(0));
    if crusher_bits_i != p.last_crusher_bits {
        p.av.set_crusher_bits(crusher_bits_i as f32);
        p.last_crusher_bits = crusher_bits_i;
    }
    if let Some(v) = read_float(p.crusher_rate) {
        if (v - p.last_crusher_rate).abs() > 1e-4 {
            p.av.set_crusher_rate(v);
            p.last_crusher_rate = v;
        }
    }

    // Sub oscillator.
    let sub_on_i = read_int(p.sub_on, p.last_sub_on.max(0));
    if sub_on_i != p.last_sub_on {
        p.av.set_sub_enabled(sub_on_i != 0);
        p.last_sub_on = sub_on_i;
    }
    if let Some(v) = read_float(p.sub_level) {
        if (v - p.last_sub_level).abs() > 1e-4 {
            p.av.set_sub_level(v);
            p.last_sub_level = v;
        }
    }
}

unsafe fn read_int(ptr: *const f32, fallback: i32) -> i32 {
    if ptr.is_null() {
        fallback
    } else {
        (*ptr).round() as i32
    }
}

unsafe fn read_float(ptr: *const f32) -> Option<f32> {
    if ptr.is_null() {
        None
    } else {
        Some(*ptr)
    }
}

unsafe extern "C" fn deactivate(_instance: LV2_Handle) {}

unsafe extern "C" fn cleanup(instance: LV2_Handle) {
    if !instance.is_null() {
        drop(Box::from_raw(instance as *mut Plugin));
    }
}

unsafe extern "C" fn extension_data(_uri: *const c_char) -> *const c_void {
    ptr::null()
}

// `LV2_Descriptor` holds raw pointers (URI is a pointer into a static
// byte array; the function pointers point to our `extern "C" fn`s) so
// it's not `Sync` by default. Every field is effectively const after
// program init, so wrapping it and declaring Sync is sound.
struct DescriptorWrapper(LV2_Descriptor);
unsafe impl Sync for DescriptorWrapper {}

static DESCRIPTOR: DescriptorWrapper = DescriptorWrapper(LV2_Descriptor {
    uri: PLUGIN_URI.as_ptr() as *const c_char,
    instantiate: Some(instantiate),
    connect_port: Some(connect_port),
    activate: Some(activate),
    run: Some(run),
    deactivate: Some(deactivate),
    cleanup: Some(cleanup),
    extension_data: Some(extension_data),
});

/// Host discovers this symbol via dlsym.
#[no_mangle]
pub unsafe extern "C" fn lv2_descriptor(index: u32) -> *const LV2_Descriptor {
    match index {
        0 => &DESCRIPTOR.0,
        _ => ptr::null(),
    }
}

// Exposed for testing/diagnostics only.
pub fn plugin_uri() -> &'static str {
    CStr::from_bytes_with_nul(PLUGIN_URI)
        .expect("valid CStr")
        .to_str()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_has_uri() {
        assert!(plugin_uri().starts_with("https://"));
    }

    #[test]
    fn lv2_descriptor_roundtrip() {
        unsafe {
            let d = lv2_descriptor(0);
            assert!(!d.is_null());
            let none = lv2_descriptor(1);
            assert!(none.is_null());
        }
    }

    #[test]
    fn instantiate_run_cleanup() {
        unsafe {
            let handle = instantiate(ptr::null(), 48_000.0, ptr::null(), ptr::null());
            assert!(!handle.is_null());
            // Connect audio ports with some scratch buffers.
            let mut inp = vec![0.1f32; 128];
            let mut outp = vec![0.0f32; 128];
            connect_port(handle, PORT_IN, inp.as_mut_ptr() as *mut c_void);
            connect_port(handle, PORT_OUT, outp.as_mut_ptr() as *mut c_void);
            // Control ports left unconnected — should fall back gracefully.
            activate(handle);
            run(handle, 128);
            deactivate(handle);
            cleanup(handle);
        }
    }
}
