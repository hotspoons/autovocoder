//! Live audio subcommand: mic → autovocoder → speakers via cpal.
//!
//! Architecture:
//!   [input stream cb]  mic samples → SPSC ringbuffer → [output stream cb]
//!                                                      AutoVocoder::process
//!                                                      → speakers
//!
//! The AutoVocoder is owned by the output callback; it pulls mic samples
//! from the ringbuffer and emits vocoded samples synchronously. No locks
//! on the audio path. Input/output sample rates must match — if they
//! don't, we bail (resampling is a future upgrade, not needed on typical
//! integrated audio where mic + speakers share a clock).

use anyhow::{bail, Context, Result};
use autovocoder_dsp::AutoVocoder;
use clap::Args;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::SharedCfg;

#[derive(Args, Debug)]
pub struct LiveArgs {
    #[command(flatten)]
    pub shared: SharedCfg,

    /// Preferred block size in frames. Smaller = lower latency, higher CPU.
    #[arg(long, default_value_t = 512)]
    pub block: u32,

    /// Input device name (substring match). Defaults to system default.
    #[arg(long)]
    pub input_device: Option<String>,

    /// Output device name (substring match). Defaults to system default.
    #[arg(long)]
    pub output_device: Option<String>,
}

pub fn run(args: LiveArgs) -> Result<()> {
    eprintln!();
    eprintln!("  ⚠  USE HEADPHONES — live vocoded output fed back into the mic");
    eprintln!("     produces nasty feedback loops very quickly.");
    eprintln!();

    let host = cpal::default_host();

    let in_dev = pick_device(&host, args.input_device.as_deref(), Direction::Input)?;
    let out_dev = pick_device(&host, args.output_device.as_deref(), Direction::Output)?;

    let in_cfg = in_dev
        .default_input_config()
        .context("default input config")?;
    let out_cfg = out_dev
        .default_output_config()
        .context("default output config")?;

    if in_cfg.sample_rate() != out_cfg.sample_rate() {
        bail!(
            "input ({} Hz) and output ({} Hz) sample rates differ — \
             not handled yet (would need resampling).",
            in_cfg.sample_rate().0,
            out_cfg.sample_rate().0
        );
    }
    let sample_rate = in_cfg.sample_rate().0 as f32;
    let in_channels = in_cfg.channels() as usize;
    let out_channels = out_cfg.channels() as usize;

    eprintln!(
        "input:  {} ({} ch, {} Hz, {:?})",
        in_dev.name().unwrap_or_default(),
        in_channels,
        in_cfg.sample_rate().0,
        in_cfg.sample_format(),
    );
    eprintln!(
        "output: {} ({} ch, {} Hz, {:?})",
        out_dev.name().unwrap_or_default(),
        out_channels,
        out_cfg.sample_rate().0,
        out_cfg.sample_format(),
    );

    // Require f32 for now — every modern CoreAudio/WASAPI path supports it.
    if in_cfg.sample_format() != SampleFormat::F32 || out_cfg.sample_format() != SampleFormat::F32 {
        bail!(
            "expected f32 sample format on both streams, got in={:?} out={:?}",
            in_cfg.sample_format(),
            out_cfg.sample_format()
        );
    }

    // Ringbuffer sized for ~100ms of mono audio — absorbs scheduling jitter.
    let rb_cap = (sample_rate as usize / 10).max(4096);
    let rb = HeapRb::<f32>::new(rb_cap);
    let (mut producer, mut consumer) = rb.split();

    let mut av = AutoVocoder::new(sample_rate, args.shared.to_config());

    let stop = Arc::new(AtomicBool::new(false));
    let stop_sig = stop.clone();
    ctrlc::set_handler(move || stop_sig.store(true, Ordering::Relaxed)).ok();

    let in_stream_cfg: StreamConfig = StreamConfig {
        channels: in_cfg.channels(),
        sample_rate: in_cfg.sample_rate(),
        buffer_size: cpal::BufferSize::Fixed(args.block),
    };
    let out_stream_cfg: StreamConfig = StreamConfig {
        channels: out_cfg.channels(),
        sample_rate: out_cfg.sample_rate(),
        buffer_size: cpal::BufferSize::Fixed(args.block),
    };

    // Input callback: sum to mono, push into ringbuffer.
    let in_err = |e| eprintln!("input stream error: {e}");
    let in_stream = in_dev
        .build_input_stream(
            &in_stream_cfg,
            move |data: &[f32], _| {
                let mut i = 0;
                while i + in_channels <= data.len() {
                    let mut s = 0.0;
                    for c in 0..in_channels {
                        s += data[i + c];
                    }
                    let _ = producer.try_push(s / in_channels as f32);
                    i += in_channels;
                }
            },
            in_err,
            None,
        )
        .context("build input stream")?;

    // Output callback: pull one mono sample per frame, run through AV, fan out.
    let out_err = |e| eprintln!("output stream error: {e}");
    let out_stream = out_dev
        .build_output_stream(
            &out_stream_cfg,
            move |data: &mut [f32], _| {
                let frames = data.len() / out_channels;
                for f in 0..frames {
                    let input = consumer.try_pop().unwrap_or(0.0);
                    let y = av.process_sample(input);
                    for c in 0..out_channels {
                        data[f * out_channels + c] = y;
                    }
                }
            },
            out_err,
            None,
        )
        .context("build output stream")?;

    in_stream.play().context("start input")?;
    out_stream.play().context("start output")?;

    eprintln!("running — Ctrl-C to stop");
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!("stopping");
    Ok(())
}

enum Direction {
    Input,
    Output,
}

fn pick_device(
    host: &cpal::Host,
    name_match: Option<&str>,
    dir: Direction,
) -> Result<cpal::Device> {
    if let Some(needle) = name_match {
        let iter: Box<dyn Iterator<Item = cpal::Device>> = match dir {
            Direction::Input => Box::new(host.input_devices()?),
            Direction::Output => Box::new(host.output_devices()?),
        };
        for d in iter {
            if d.name().map(|n| n.contains(needle)).unwrap_or(false) {
                return Ok(d);
            }
        }
        bail!("no device matching {:?}", needle);
    }
    match dir {
        Direction::Input => host
            .default_input_device()
            .context("no default input device"),
        Direction::Output => host
            .default_output_device()
            .context("no default output device"),
    }
}
