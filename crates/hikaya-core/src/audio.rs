//! Small audio helpers for capture paths that do not go through Handy's
//! recorder (system audio on Windows): mixing to mono and converting to the
//! 16 kHz the speech models expect.

/// Mixes interleaved samples with `channels` channels down to mono.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Frames this long (at 16 kHz) are judged loud or quiet one at a time.
const FRAME: usize = 480;
/// A frame louder than this (RMS, about -46 dBFS) counts as sound.
const LOUD_RMS: f32 = 0.005;
/// Least amount of sound, in frames (about 0.3 s), for a stretch to be
/// worth transcribing.
const MIN_LOUD_FRAMES: usize = 10;

/// True when 16 kHz mono `samples` contain enough sound to be worth sending
/// to a speech model. Whisper invents words ("you", "Thank you.") for silent
/// or near-silent audio, so silence must not reach it.
pub fn has_speech(samples: &[f32]) -> bool {
    samples
        .chunks(FRAME)
        .filter(|frame| {
            let energy: f32 = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
            energy.sqrt() > LOUD_RMS
        })
        .take(MIN_LOUD_FRAMES)
        .count()
        >= MIN_LOUD_FRAMES
}

/// Converts a mono stream from one sample rate to another, chunk by chunk.
///
/// When reducing the rate, each output sample is the average of the input
/// samples it covers, which removes most of the high frequencies that would
/// otherwise fold back into the speech band. Good enough for transcription;
/// not meant for music.
#[derive(Debug, Clone)]
pub struct Resampler {
    from: u32,
    to: u32,
    /// Position of the next output sample, in input samples, relative to the
    /// start of `pending`.
    position: f64,
    /// Input samples not yet fully consumed.
    pending: Vec<f32>,
}

impl Resampler {
    /// A resampler from `from` Hz to `to` Hz.
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            from: from.max(1),
            to: to.max(1),
            position: 0.0,
            pending: Vec::new(),
        }
    }

    /// Input samples per output sample.
    fn step(&self) -> f64 {
        f64::from(self.from) / f64::from(self.to)
    }

    /// Feeds input samples and returns the output samples now available.
    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if self.from == self.to {
            return input.to_vec();
        }
        self.pending.extend_from_slice(input);
        let step = self.step();
        let mut out = Vec::with_capacity((input.len() as f64 / step) as usize + 1);
        loop {
            let start = self.position;
            let end = start + step;
            // Need the sample after `end` to interpolate when upsampling.
            if end.ceil() as usize >= self.pending.len() {
                break;
            }
            out.push(if step > 1.0 {
                self.average(start, end)
            } else {
                self.interpolate(start)
            });
            self.position = end;
        }
        // Drop what is fully consumed, keeping positions relative.
        let consumed = self.position.floor() as usize;
        self.pending.drain(..consumed.min(self.pending.len()));
        self.position -= consumed as f64;
        out
    }

    fn interpolate(&self, at: f64) -> f32 {
        let i = at.floor() as usize;
        let frac = (at - at.floor()) as f32;
        let a = self.pending[i];
        let b = self.pending.get(i + 1).copied().unwrap_or(a);
        a + (b - a) * frac
    }

    fn average(&self, start: f64, end: f64) -> f32 {
        let (first, last) = (
            start.floor() as usize,
            (end.ceil() as usize).min(self.pending.len()),
        );
        let mut sum = 0.0f64;
        let mut weight = 0.0f64;
        for i in first..last {
            let lo = (i as f64).max(start);
            let hi = ((i + 1) as f64).min(end);
            if hi > lo {
                sum += f64::from(self.pending[i]) * (hi - lo);
                weight += hi - lo;
            }
        }
        if weight > 0.0 {
            (sum / weight) as f32
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    /// Rough frequency estimate from zero crossings.
    fn frequency(samples: &[f32], rate: u32) -> f32 {
        let crossings = samples
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        crossings as f32 * rate as f32 / samples.len() as f32
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), [0.5, 0.5]);
        assert_eq!(downmix(&[0.2, 0.4], 1), [0.2, 0.4]);
        // A trailing partial frame is dropped.
        assert_eq!(downmix(&[1.0, 1.0, 1.0], 2), [1.0]);
    }

    #[test]
    fn silence_and_a_click_are_not_speech() {
        assert!(!has_speech(&[]));
        assert!(!has_speech(&vec![0.0; 16_000 * 30]));
        // Background hiss well below speech level.
        let hiss: Vec<f32> = (0..16_000u64 * 30)
            .map(|i: u64| ((i * 7919 % 101) as f32 / 101.0 - 0.5) * 0.004)
            .collect();
        assert!(!has_speech(&hiss));
        // A single loud click.
        let mut click = vec![0.0; 16_000 * 30];
        click[1000..1200].fill(0.8);
        assert!(!has_speech(&click));
    }

    #[test]
    fn a_second_of_voice_level_sound_is_speech() {
        let mut audio = vec![0.0; 16_000 * 30];
        let voice = sine(16_000, 220.0, 1.0);
        audio[160_000..176_000].copy_from_slice(&voice.iter().map(|s| s * 0.1).collect::<Vec<_>>());
        assert!(has_speech(&audio));
    }

    #[test]
    fn same_rate_is_a_copy() {
        let mut r = Resampler::new(16_000, 16_000);
        assert_eq!(r.push(&[0.1, 0.2]), [0.1, 0.2]);
    }

    #[test]
    fn length_follows_the_rate_ratio() {
        for (from, to) in [
            (48_000u32, 16_000u32),
            (44_100, 16_000),
            (8_000, 16_000),
            (22_050, 16_000),
        ] {
            let mut r = Resampler::new(from, to);
            let input = sine(from, 440.0, 2.0);
            // Fed in uneven chunks, as audio callbacks do.
            let out: Vec<f32> = input.chunks(997).flat_map(|c| r.push(c)).collect();
            let expected = input.len() as f64 * f64::from(to) / f64::from(from);
            assert!(
                (out.len() as f64 - expected).abs() <= 3.0,
                "{from}->{to}: {} vs {expected}",
                out.len()
            );
        }
    }

    #[test]
    fn a_tone_keeps_its_pitch() {
        for from in [48_000u32, 44_100, 8_000] {
            let mut r = Resampler::new(from, 16_000);
            let out: Vec<f32> = sine(from, 440.0, 1.0)
                .chunks(512)
                .flat_map(|c| r.push(c))
                .collect();
            let f = frequency(&out, 16_000);
            assert!((f - 440.0).abs() < 10.0, "from {from}: {f} Hz");
        }
    }

    #[test]
    fn downsampling_attenuates_what_would_alias() {
        // 15 kHz cannot be represented at 16 kHz; averaging must shrink it a lot.
        let mut r = Resampler::new(48_000, 16_000);
        let out = r.push(&sine(48_000, 15_000.0, 0.5));
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak < 0.5, "peak {peak}");
    }

    proptest::proptest! {
        #[test]
        fn never_panics_and_stays_in_range(
            chunks in proptest::collection::vec(proptest::collection::vec(-1.0f32..1.0, 0..300), 0..20),
            from in 4_000u32..96_000,
        ) {
            let mut r = Resampler::new(from, 16_000);
            for chunk in &chunks {
                for s in r.push(chunk) {
                    proptest::prop_assert!((-1.0001..=1.0001).contains(&s));
                }
            }
        }
    }
}
