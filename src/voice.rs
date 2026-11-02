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

    /// Stops recording; returns mono samples at `target_rate`.
    pub fn finish(self, target_rate: u32) -> Vec<f32> {
        let samples = std::mem::take(&mut *self.samples.lock().unwrap_or_else(|p| p.into_inner()));
        resample(&samples, self.rate, target_rate)
    }
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
