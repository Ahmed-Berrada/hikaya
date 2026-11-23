//! Diarizes a 16 kHz WAV with speakrs (pyannote community-1) and prints RTTM
//! on stdout, with the speed on stderr. Used by ADR 0005's measurements.
//!
//!     cargo run --release -- ES2004a.wav ES2004a > ES2004a.rttm
//!     cargo run --release --no-default-features --features cuda -- ES2004a.wav ES2004a
//!
//! Models are downloaded from Hugging Face (avencera/speakrs-models) on
//! first use.
use speakrs::{ExecutionMode, OwnedDiarizationPipeline};
use std::time::Instant;

#[cfg(feature = "cuda")]
const MODE: ExecutionMode = ExecutionMode::Cuda;
#[cfg(not(feature = "cuda"))]
const MODE: ExecutionMode = ExecutionMode::Cpu;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Mono samples of a 16 kHz WAV file.
fn read(path: &str) -> Result<Vec<f32>, Error> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.sample_rate != 16_000 {
        return Err(format!("{path} is {} Hz; 16 kHz is expected", spec.sample_rate).into());
    }
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()?
        }
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
    };
    let channels = usize::from(spec.channels);
    Ok(raw
        .chunks(channels)
        .map(|c| c.iter().sum::<f32>() / channels as f32)
        .collect())
}

fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: hikaya-diarize-eval <file.wav> [id]")?;
    let id = args.next().unwrap_or_else(|| "meeting".into());
    let audio = read(&path)?;
    let mut pipeline = OwnedDiarizationPipeline::from_pretrained(MODE)?;
    let start = Instant::now();
    let result = pipeline.run(&audio)?;
    let seconds = audio.len() as f64 / 16_000.0;
    let took = start.elapsed().as_secs_f64();
    eprintln!(
        "{id} ({MODE:?}): {seconds:.0} s of audio in {took:.1} s = {:.1}x real time",
        seconds / took
    );
    print!("{}", result.rttm(&id));
    Ok(())
}
