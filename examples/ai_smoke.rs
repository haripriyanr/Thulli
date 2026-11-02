//! End-to-end check of the on-device AI: downloads the model pack the way the
//! app does, then runs the planner, speech recognition and speech once.
//!
//! cargo run --release --example ai_smoke -- [english-test.wav]

use std::time::Instant;

use thulli_core::ai::{asr, pack, tts, Models};
use thulli_core::services::assistant::{ChatEngine, EngineMessage, EngineRole};

fn main() {
    let models = Models::default();
    let t = Instant::now();
    let mut last = String::new();
    let mut progress = |p: pack::PackProgress| {
        let line = p.describe();
        if line != last {
            println!("  {line}");
            last = line;
        }
    };
    let dir = pack::ensure(&mut progress).expect("model pack");
    println!("pack ready in {:.0?} at {}", t.elapsed(), dir.display());

    // Planner
    let t = Instant::now();
    let llm = models.llm(&mut progress).expect("planner");
    println!("planner loaded in {:.1?}", t.elapsed());
    let store = thulli_core::models::AppStateStore::default();
    let prompt = thulli_core::services::assistant::plan_messages(
        &store,
        "I have a DSP exam with 7 modules, make a 12 hour plan to study it and add tasks accordingly",
    );
    let t = Instant::now();
    let mut tokens = 0usize;
    let reply = llm
        .generate(&prompt, &mut |_| {
            tokens += 1;
            true
        })
        .expect("planner reply");
    println!("planner: {tokens} pieces in {:.1?}", t.elapsed());
    let answer = thulli_core::ai::llm::strip_thinking(&reply);
    println!("--- answer ---\n{}\n--------------", &answer[..answer.len().min(1500)]);
    let parsed = thulli_core::services::assistant::parse_reply(answer);
    println!("parsed actions: {} (rejected {:?})", parsed.actions.len(), parsed.rejected);

    // Speech recognition
    if let Some(path) = std::env::args().nth(1) {
        let samples = read_wav_16k(&path);
        let asr = models.asr(&mut progress).expect("asr");
        let t = Instant::now();
        println!("asr: {:?} in {:.1?}", asr.transcribe(&samples).expect("transcribe"), t.elapsed());
    }
    let _ = asr::SAMPLE_RATE;

    // Speech
    let tts = models.tts(&mut progress).expect("tts");
    let t = Instant::now();
    let wave = tts.speak("Your study plan is ready. Seven modules, twelve hours.").expect("speak");
    println!("tts: {:.1} s of audio in {:.1?}", wave.len() as f32 / tts::SAMPLE_RATE as f32, t.elapsed());
    write_wav("tts_smoke.wav", &wave, tts::SAMPLE_RATE);
    println!("wrote tts_smoke.wav");
    let _ = EngineMessage::new(EngineRole::User, "");
}

/// Reads a 16-bit PCM mono/stereo WAV and returns 16 kHz mono samples.
fn read_wav_16k(path: &str) -> Vec<f32> {
    let b = std::fs::read(path).expect("wav");
    let u16le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32le = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (mut pos, mut channels, mut rate) = (12, 1usize, 16_000u32);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32le(pos + 4) as usize;
        if id == b"fmt " {
            channels = u16le(pos + 10) as usize;
            rate = u32le(pos + 12);
        } else if id == b"data" {
            let data = &b[pos + 8..(pos + 8 + size).min(b.len())];
            let mono: Vec<f32> = data
                .chunks_exact(2 * channels)
                .map(|f| f.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0).sum::<f32>() / channels as f32)
                .collect();
            let ratio = rate as f64 / 16_000.0;
            return (0..(mono.len() as f64 / ratio) as usize).map(|i| mono[(i as f64 * ratio) as usize]).collect();
        }
        pos += 8 + size + (size & 1);
    }
    panic!("no data chunk");
}

fn write_wav(path: &str, samples: &[f32], rate: u32) {
    let data: Vec<u8> = samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((36 + data.len() as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    std::fs::write(path, out).expect("write wav");
}
