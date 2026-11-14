//! The microphone and the speaker for on-device speech. [`Recorder`] keeps
//! the system's default microphone open while the mic button is on and hands
//! back mono 16 kHz samples for speech recognition. [`play`] speaks a
//! waveform through the default output device.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// An open microphone. Dropping it closes the device.
pub struct Recorder {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    rate: u32,
}

impl Recorder {
    /// Starts recording from the default microphone.
    pub fn start() -> Result<Self, String> {
        let device = cpal::default_host().default_input_device().ok_or("no microphone was found")?;
        let config = device.default_input_config().map_err(|e| format!("the microphone could not be opened: {e}"))?;
        let channels = config.channels() as usize;
        let rate = config.sample_rate();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let sink = samples.clone();
        let err = |e| eprintln!("microphone: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                config.clone().into(),
                move |data: &[f32], _: &_| push_mono(&sink, data.iter().copied(), channels),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                config.clone().into(),
                move |data: &[i16], _: &_| push_mono(&sink, data.iter().map(|&s| s as f32 / 32768.0), channels),
                err,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                config.clone().into(),
                move |data: &[u16], _: &_| push_mono(&sink, data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0), channels),
                err,
                None,
            ),
            other => return Err(format!("the microphone uses an unsupported format ({other:?})")),
        }
        .map_err(|e| format!("the microphone could not be opened: {e}"))?;
        stream.play().map_err(|e| format!("the microphone could not start: {e}"))?;
        Ok(Self { _stream: stream, samples, rate })
    }

    /// How loud the last moments were, 0 to 1, for a live level display.
    pub fn level(&self) -> f32 {
        let samples = self.samples.lock().unwrap_or_else(|p| p.into_inner());
        let window = (self.rate as usize / 20).max(1);
        let start = samples.len().saturating_sub(window);
        level_of(&samples[start..])
    }

    /// Stops recording; returns mono samples at `target_rate`.
    pub fn finish(self, target_rate: u32) -> Vec<f32> {
        let samples = std::mem::take(&mut *self.samples.lock().unwrap_or_else(|p| p.into_inner()));
        resample(&samples, self.rate, target_rate)
    }
}

/// Loudness of a short stretch of speech as 0 to 1 (normal speech sits near 0.05 to 0.2 RMS).
fn level_of(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    (rms * 8.0).min(1.0)
}

/// Averages interleaved channels into one.
fn push_mono(sink: &Mutex<Vec<f32>>, data: impl Iterator<Item = f32>, channels: usize) {
    let data: Vec<f32> = data.collect();
    let mut out = sink.lock().unwrap_or_else(|p| p.into_inner());
    out.extend(data.chunks(channels.max(1)).map(|frame| frame.iter().sum::<f32>() / frame.len() as f32));
}

/// Linear resampling; enough for speech recognition.
fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let len = (input.len() as f64 / ratio) as usize;
    (0..len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let j = pos as usize;
            let frac = (pos - j as f64) as f32;
            let a = input[j];
            let b = *input.get(j + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

/// Plays mono samples on the default output device, blocking until done.
/// Call it from a worker thread.
pub fn play(samples: Vec<f32>, rate: u32) -> Result<(), String> {
    if samples.is_empty() {
        return Ok(());
    }
    let mut sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| format!("no speaker was found: {e}"))?;
    sink.log_on_drop(false);
    let player = rodio::Player::connect_new(sink.mixer());
    player.append(rodio::buffer::SamplesBuffer::new(std::num::NonZero::new(1).unwrap(), std::num::NonZero::new(rate).ok_or("bad sample rate")?, samples));
    player.sleep_until_end();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_duration() {
        let one_second = vec![0.5f32; 48_000];
        assert_eq!(resample(&one_second, 48_000, 16_000).len(), 16_000);
        assert_eq!(resample(&one_second, 16_000, 16_000).len(), 48_000);
    }

    #[test]
    fn channels_are_averaged() {
        let sink = Mutex::new(Vec::new());
        push_mono(&sink, [1.0, 0.0, 0.5, 0.5].into_iter(), 2);
        assert_eq!(*sink.lock().unwrap(), vec![0.5, 0.5]);
    }
}

#[cfg(test)]
mod level_tests {
    use super::level_of;

    #[test]
    fn silence_is_zero_and_loud_is_capped() {
        assert_eq!(level_of(&[]), 0.0);
        assert_eq!(level_of(&[0.0; 100]), 0.0);
        assert_eq!(level_of(&[1.0; 100]), 1.0);
        let speech = level_of(&[0.1, -0.1, 0.1, -0.1]);
        assert!(speech > 0.5 && speech < 1.0);
    }
}
