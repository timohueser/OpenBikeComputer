//! The simulator's [`Sounder`]: each cue plays through the computer's sound card, so a developer
//! hears what the rider hears.
//!
//! Only the window opens a stream. The headless `--png` path and the tests never build one, so
//! they need no audio device.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use obc_ports::{Note, Sounder, Volume};

/// The rendered cue and how far the device has played it.
#[derive(Default)]
struct Playback {
    samples: Vec<f32>,
    next: usize,
}

impl Playback {
    fn next_sample(&mut self) -> f32 {
        let sample = self.samples.get(self.next).copied().unwrap_or(0.0);
        self.next = (self.next + 1).min(self.samples.len());
        sample
    }
}

pub struct SimSounder {
    /// The open output stream and its sample rate. `None` is a silent platform: `--no-sound`, or
    /// no device opened.
    stream: Option<(cpal::Stream, u32)>,
    playback: Arc<Mutex<Playback>>,
}

impl SimSounder {
    /// Open the default output device, or stay silent when `enabled` is false. A device that does
    /// not open costs one line on stderr, never the session.
    pub fn open(enabled: bool) -> Self {
        let playback = Arc::default();
        let stream = enabled
            .then(|| start(&playback))
            .and_then(|opened| opened.map_err(|e| eprintln!("obc-sim: no sound output ({e}); cues stay silent")).ok());
        SimSounder { stream, playback }
    }
}

impl Sounder for SimSounder {
    fn available(&self) -> bool {
        self.stream.is_some()
    }

    fn play(&mut self, notes: &'static [Note], volume: Volume) {
        let Some((_, rate)) = &self.stream else { return };
        let mut cue = Playback { samples: obc_host_core::tone::render(notes, volume, *rate), next: 0 };
        // The old samples leave under the lock and drop after it, on this thread, so the audio
        // callback never frees memory.
        if let Ok(mut playing) = self.playback.lock() {
            std::mem::swap(&mut *playing, &mut cue);
        }
    }
}

fn start(playback: &Arc<Mutex<Playback>>) -> Result<(cpal::Stream, u32), cpal::Error> {
    let device = cpal::default_host().default_output_device().ok_or(cpal::ErrorKind::DeviceNotAvailable)?;
    let config = device.default_output_config()?;
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build::<f32>(&device, config.config(), playback),
        cpal::SampleFormat::I16 => build::<i16>(&device, config.config(), playback),
        cpal::SampleFormat::I32 => build::<i32>(&device, config.config(), playback),
        cpal::SampleFormat::U16 => build::<u16>(&device, config.config(), playback),
        _ => Err(cpal::ErrorKind::UnsupportedConfig.into()),
    }?;
    stream.play()?;
    Ok((stream, config.sample_rate()))
}

/// The callback does not allocate and does not wait: while the GUI thread holds the lock to swap
/// in a new cue, that one buffer plays silence.
fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    playback: &Arc<Mutex<Playback>>,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = usize::from(config.channels);
    let playback = Arc::clone(playback);
    device.build_output_stream(
        config,
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut playing = playback.try_lock();
            for frame in out.chunks_mut(channels) {
                let sample = playing.as_deref_mut().map_or(0.0, Playback::next_sample);
                frame.fill(T::from_sample(sample));
            }
        },
        |e| eprintln!("obc-sim: sound: {e}"),
        None,
    )
}
