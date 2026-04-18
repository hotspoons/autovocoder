//! LV2 plugin wrapper around `autovocoder-dsp`.
//!
//! This crate intentionally avoids the `lv2-rs` bindings family to keep the
//! dep tree tiny and the ABI transparent. The LV2 C ABI is simple and stable;
//! we define only the types we actually use below.
//!
//! Ports (see lv2/autovocoder.ttl for matching symbol names and ranges):
//!   0  AudioInput
//!   1  AudioOutput
//!   2  Mode         (int: 0=mono, 1=major_triad, 2=minor_triad, 3=fixed)
//!   3  FixedNote    (int MIDI 0..127, used when Mode==fixed)
//!   4  Scale        (int: 0=chromatic, 1=major, 2=minor)
//!   5  ScaleRoot    (int pitch class 0..11)
//!   6  Mix          (float 0..1, dry/wet)
//!   7  Portamento   (float 1..500 ms)
//!   8  CarrierLevel (float 0..2)
//!   9  OutputGain   (float -20..+30 dB; post-compressor makeup)
//!  10  CompOn       (int 0/1; enable the built-in compressor)
//!  11  CompThreshold (float -40..0 dB; compressor threshold)

#![allow(non_camel_case_types)]
#![allow(clippy::missing_safety_doc)]

use autovocoder_dsp::{AutoVocoder, AutoVocoderConfig, CarrierMode, Scale};
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

const PLUGIN_URI: &[u8] = b"https://github.com/richsio/autovocoder\0";

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
const PORT_OUTPUT_GAIN: u32 = 9;
const PORT_COMP_ON: u32 = 10;
const PORT_COMP_THRESHOLD: u32 = 11;

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
    output_gain: *const f32,
    comp_on: *const f32,
    comp_threshold: *const f32,

    // Last-seen values, so we only push into DSP on change (cheap RT-safe).
    last_mode: i32,
    last_fixed_note: i32,
    last_scale_kind: i32,
    last_scale_root: i32,
    last_mix: f32,
    last_portamento: f32,
    last_carrier_level: f32,
    last_output_gain: f32,
    last_comp_on: i32,
    last_comp_threshold: f32,
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
        output_gain: ptr::null(),
        comp_on: ptr::null(),
        comp_threshold: ptr::null(),
        last_mode: -1,
        last_fixed_note: -1,
        last_scale_kind: -1,
        last_scale_root: -1,
        last_mix: f32::NAN,
        last_portamento: f32::NAN,
        last_carrier_level: f32::NAN,
        last_output_gain: f32::NAN,
        last_comp_on: -1,
        last_comp_threshold: f32::NAN,
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
        PORT_OUTPUT_GAIN => p.output_gain = data as *const f32,
        PORT_COMP_ON => p.comp_on = data as *const f32,
        PORT_COMP_THRESHOLD => p.comp_threshold = data as *const f32,
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
    for i in 0..n {
        output[i] = p.av.process_sample(input[i]);
    }
}

unsafe fn apply_controls(p: &mut Plugin) {
    // Each control port is an f32 written by the host. Null = host didn't
    // connect it; fall back to current DSP defaults.
    let mode_i = read_int(p.mode, p.last_mode);
    let fixed_i = read_int(p.fixed_note, p.last_fixed_note.max(0));
    let scale_i = read_int(p.scale_kind, p.last_scale_kind);
    let root_i = read_int(p.scale_root, p.last_scale_root.max(0));

    if mode_i != p.last_mode || fixed_i != p.last_fixed_note {
        p.av.set_carrier_mode(match mode_i {
            1 => CarrierMode::major_triad(),
            2 => CarrierMode::minor_triad(),
            3 => CarrierMode::Fixed {
                midi: fixed_i.clamp(0, 127) as u8,
            },
            _ => CarrierMode::Mono,
        });
        p.last_mode = mode_i;
        p.last_fixed_note = fixed_i;
    }

    if scale_i != p.last_scale_kind || root_i != p.last_scale_root {
        let root_pc = root_i.rem_euclid(12) as u8;
        p.av.set_scale(match scale_i {
            1 => Scale::major(root_pc),
            2 => Scale::minor(root_pc),
            _ => Scale::CHROMATIC,
        });
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
