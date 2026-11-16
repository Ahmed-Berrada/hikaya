//! Opening the two meeting tracks.
//!
//! - Microphone: `handy-recorder`, the same as dictation, using the
//!   microphone chosen in settings.
//! - System audio on Linux: the PipeWire/PulseAudio monitor of the default
//!   output, also through `handy-recorder`.
//! - System audio on Windows: WASAPI loopback of the default output, through
//!   cpal directly (opening an output device for input is how WASAPI does
//!   loopback). This path is compiled in CI and checked by hand on Windows.
//!
//! Every track delivers 16 kHz mono `Vec<f32>` chunks on a channel.
//!
//! This is its own crate so CI can lint the Windows path from Linux:
//! `cargo clippy --target x86_64-pc-windows-msvc`.

use anyhow::{anyhow, Context, Result};
use handy_recorder::{AudioChunk, Recorder, RecorderConfig, Sink};
use std::sync::mpsc::Sender;

/// Sample rate every track delivers.
pub const SAMPLE_RATE: u32 = 16_000;

/// A running capture. Stopping it ends the track; dropping it does too.
pub struct TrackCapture {
    stop: Option<Box<dyn FnOnce() + Send>>,
    /// Human-readable device name, for the log and the UI.
    pub device_name: String,
}

impl TrackCapture {
    /// Stops capturing. The channel's sender is dropped once the last audio
    /// has been delivered.
    pub fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

impl Drop for TrackCapture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

/// Forwards each chunk's real samples to a channel.
struct ChannelSink(Sender<Vec<f32>>);

impl Sink for ChannelSink {
    fn process_chunk(&mut self, chunk: AudioChunk<'_>) {
        let real = chunk.valid_frames * usize::from(chunk.channels);
        // A closed channel means the meeting is ending; nothing to do.
        let _ = self.0.send(chunk.samples[..real].to_vec());
    }
}

fn start_recorder(
    device: Option<String>,
    device_name: String,
    tx: Sender<Vec<f32>>,
) -> Result<TrackCapture> {
    let config = RecorderConfig {
        device,
        ..RecorderConfig::speech()
    };
    let recorder: Recorder<ChannelSink> = Recorder::open(config).context("opening the device")?;
    recorder
        .start(ChannelSink(tx))
        .map_err(|e| anyhow!("starting the recording: {e}"))?;
    Ok(TrackCapture {
        stop: Some(Box::new(move || {
            // Dropping the stopped sink drops the sender, which ends the track.
            if let Err(e) = recorder.stop() {
                log::warn!("Meeting track stopped with an error: {e}");
            }
            drop(recorder);
        })),
        device_name,
    })
}

/// Starts the microphone track. `selected` is the microphone name chosen in
/// settings; `None`, or a name that is no longer connected, uses the default.
pub fn start_microphone(selected: Option<&str>, tx: Sender<Vec<f32>>) -> Result<TrackCapture> {
    let devices = handy_recorder::list_input_devices().unwrap_or_default();
    let chosen = selected.and_then(|name| {
        devices
            .iter()
            .find(|d| !d.is_monitor && d.name == name)
            .cloned()
    });
    let name = chosen
        .as_ref()
        .map(|d| d.name.clone())
        .or_else(|| {
            devices
                .iter()
                .find(|d| d.is_default)
                .map(|d| d.name.clone())
        })
        .unwrap_or_else(|| "default microphone".to_string());
    start_recorder(chosen.map(|d| d.id), name, tx)
}

/// Starts the system-audio track: what the computer plays, which during a
/// call is the other people.
#[cfg(target_os = "linux")]
pub fn start_system_audio(tx: Sender<Vec<f32>>) -> Result<TrackCapture> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let output = cpal::host_from_id(cpal::HostId::PulseAudio)
        .context("PulseAudio/PipeWire is not running")?
        .default_output_device()
        .context("there is no default audio output")?;
    let sink = output
        .id()
        .map_err(|e| anyhow!("reading the default output: {e}"))?
        .id()
        .to_string();
    let wanted = format!("pulseaudio:{sink}.monitor");
    let monitor = handy_recorder::list_input_devices()
        .map_err(|e| anyhow!("listing audio devices: {e}"))?
        .into_iter()
        .find(|d| d.is_monitor && d.id == wanted)
        .with_context(|| format!("no monitor source for the default output ({sink})"))?;
    start_recorder(Some(monitor.id), monitor.name, tx)
}

/// Starts the system-audio track: WASAPI loopback of the default output.
#[cfg(windows)]
pub fn start_system_audio(tx: Sender<Vec<f32>>) -> Result<TrackCapture> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use hikaya_core::audio::{downmix, Resampler};
    use std::sync::mpsc;

    let (ready_tx, ready_rx) = mpsc::channel::<Result<String>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    // The stream lives on its own thread: cpal streams are not always `Send`.
    let thread = std::thread::Builder::new()
        .name("hikaya-loopback".into())
        .spawn(move || {
            let opened = (|| -> Result<(cpal::Stream, String)> {
                let device = cpal::default_host()
                    .default_output_device()
                    .context("there is no default audio output")?;
                let name = device
                    .description()
                    .map(|d| d.name().to_string())
                    .unwrap_or_else(|_| "system audio".into());
                let config = device
                    .default_output_config()
                    .context("reading the output format")?;
                let channels = usize::from(config.channels());
                let rate = config.sample_rate();
                let format = config.sample_format();
                let mut resampler = Resampler::new(rate, SAMPLE_RATE);
                let on_error = |e| log::warn!("System audio stream error: {e}");
                let stream = match format {
                    cpal::SampleFormat::F32 => device.build_input_stream(
                        config.into(),
                        move |data: &[f32], _| {
                            let _ = tx.send(resampler.push(&downmix(data, channels)));
                        },
                        on_error,
                        None,
                    ),
                    cpal::SampleFormat::I16 => device.build_input_stream(
                        config.into(),
                        move |data: &[i16], _| {
                            let floats: Vec<f32> = data
                                .iter()
                                .map(|&s| f32::from(s) / f32::from(i16::MAX))
                                .collect();
                            let _ = tx.send(resampler.push(&downmix(&floats, channels)));
                        },
                        on_error,
                        None,
                    ),
                    other => return Err(anyhow!("unsupported output format {other:?}")),
                }
                .context("opening loopback capture")?;
                stream.play().context("starting loopback capture")?;
                Ok((stream, name))
            })();
            match opened {
                Ok((stream, name)) => {
                    let _ = ready_tx.send(Ok(name));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .context("starting the loopback thread")?;
    let device_name = ready_rx
        .recv()
        .map_err(|_| anyhow!("the loopback thread stopped"))??;
    Ok(TrackCapture {
        stop: Some(Box::new(move || {
            let _ = stop_tx.send(());
            let _ = thread.join();
        })),
        device_name,
    })
}

/// System audio is not captured on this platform.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn start_system_audio(_tx: Sender<Vec<f32>>) -> Result<TrackCapture> {
    Err(anyhow!(
        "system audio capture is not supported on this platform"
    ))
}
