//! Offline render subcommand: WAV in, WAV out.

use anyhow::{bail, Context, Result};
use autovocoder_dsp::AutoVocoder;
use clap::Args;
use std::path::PathBuf;

use crate::SharedCfg;

#[derive(Args, Debug)]
pub struct RenderArgs {
    /// Input WAV (mono or stereo; stereo is summed to mono).
    #[arg(short, long)]
    pub input: PathBuf,

    /// Output WAV (mono, 16-bit).
    #[arg(short, long)]
    pub output: PathBuf,

    #[command(flatten)]
    pub shared: SharedCfg,
}

pub fn run(args: RenderArgs) -> Result<()> {
    let mut reader = hound::WavReader::open(&args.input)
        .with_context(|| format!("opening {}", args.input.display()))?;
    let spec = reader.spec();
    if spec.channels != 1 && spec.channels != 2 {
        bail!(
            "only mono or stereo WAV supported, got {} channels",
            spec.channels
        );
    }
    let sample_rate = spec.sample_rate as f32;

    let samples = read_samples_as_mono_f32(&mut reader)?;
    eprintln!(
        "input: {} Hz, {} ch, {} samples ({:.2}s)",
        spec.sample_rate,
        spec.channels,
        samples.len(),
        samples.len() as f32 / sample_rate,
    );

    let cfg = args.shared.to_config();
    let mut av = AutoVocoder::new(sample_rate, cfg);

    let mut out = samples;
    av.process_buffer(&mut out);

    let out_spec = hound::WavSpec {
        channels: 1,
        sample_rate: spec.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&args.output, out_spec)
        .with_context(|| format!("creating {}", args.output.display()))?;
    for s in &out {
        let clipped = s.clamp(-1.0, 1.0);
        writer.write_sample((clipped * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;

    eprintln!("wrote {}", args.output.display());
    Ok(())
}

fn read_samples_as_mono_f32(
    reader: &mut hound::WavReader<std::io::BufReader<std::fs::File>>,
) -> Result<Vec<f32>> {
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<std::result::Result<_, _>>()?
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<_, _>>()?,
    };
    if channels == 1 {
        return Ok(samples);
    }
    let frames = samples.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut sum = 0.0;
        for c in 0..channels {
            sum += samples[f * channels + c];
        }
        mono.push(sum / channels as f32);
    }
    Ok(mono)
}
