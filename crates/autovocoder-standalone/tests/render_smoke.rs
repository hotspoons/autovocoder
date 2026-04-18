//! End-to-end smoke test: synthesize a vowel-like source, render it through
//! the CLI's public config path, and verify the output is audible.
//!
//! Kept in `tests/` (integration test) so it runs with `cargo test` but
//! doesn't pollute the binary's runtime code.

use autovocoder_dsp::{AutoVocoder, AutoVocoderConfig, CarrierMode};
use std::f32::consts::TAU;

#[test]
fn soundwave_fixed_c3_produces_audible_output() {
    let sr = 48_000.0;
    let cfg = AutoVocoderConfig {
        carrier_mode: CarrierMode::Fixed { midi: 48 }, // C3
        ..AutoVocoderConfig::default()
    };
    let mut av = AutoVocoder::new(sr, cfg);

    // 2 seconds of a buzzy "voice": sawtooth-like stack of 12 partials
    // at 160 Hz fundamental. Rich enough to excite every vocoder band.
    let n = (sr as usize) * 2;
    let mut buf: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            let mut x = 0.0;
            for h in 1..=12 {
                x += (TAU * 160.0 * h as f32 * t).sin() / h as f32;
            }
            0.4 * x
        })
        .collect();

    av.process_buffer(&mut buf);

    let tail = &buf[buf.len() / 2..];
    let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
    let peak = tail.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    assert!(rms > 0.01, "RMS too low ({rms})");
    assert!(peak < 5.0, "output unreasonably hot ({peak})");
}
