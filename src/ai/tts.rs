//! Speech with OmniVoice (int4 ONNX): a Qwen3-0.6B backbone predicts eight
//! codebooks of audio codes by iterative unmasking, and the Higgs decoder
//! turns the codes into a 24 kHz waveform.
//!
//! Port of the reference `inference.py` from onnx-community/OmniVoice-Onnx
//! (auto voice, no cloning).

use std::path::Path;
use std::sync::Mutex;

use ort::session::Session;
use ort::value::{DynValue, Tensor};
use tokenizers::Tokenizer;

/// Output sample rate.
pub const SAMPLE_RATE: u32 = 24_000;
const CODEBOOKS: usize = 8;
const MASK_ID: i64 = 1024;
const REAL_CODES: usize = 1024;
const HEAD_VOCAB: usize = 1025;
const STEPS: usize = 32;
/// Unmasking priority of each codebook.
const CB_WEIGHTS: [f32; CODEBOOKS] = [8.0, 8.0, 6.0, 6.0, 4.0, 4.0, 2.0, 2.0];
/// Audio frames per second of speech.
const FRAMES_PER_SEC: f32 = 25.0;
/// Longest single utterance, in frames (30 s).
const MAX_FRAMES: usize = 750;

pub struct Tts {
    embeddings: Mutex<Session>,
    llm: Mutex<Session>,
    heads: Mutex<Session>,
    decoder: Mutex<Session>,
    tokenizer: Tokenizer,
}

fn session(path: &Path) -> Result<Session, String> {
    super::open_session(path).map_err(|e| format!("could not load the voice model: {e}"))
}

impl Tts {
    /// Loads the four sub-models and the tokenizer from `dir`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        Ok(Self {
            embeddings: Mutex::new(session(&dir.join("audio_embeddings_encoder.onnx"))?),
            llm: Mutex::new(session(&dir.join("llm_decoder.onnx"))?),
            heads: Mutex::new(session(&dir.join("audio_heads_decoder.onnx"))?),
            decoder: Mutex::new(session(&dir.join("higgs_decoder.onnx"))?),
            tokenizer: Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| e.to_string())?,
        })
    }

    /// Speaks `text`; returns mono samples at [`SAMPLE_RATE`].
    pub fn speak(&self, text: &str) -> Result<Vec<f32>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let enc = self.tokenizer.encode(text, true).map_err(|e| e.to_string())?;
        let text_ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        let frames = estimate_frames(text);
        let codes = self.unmask(&text_ids, frames)?;
        self.decode(&codes, frames)
    }

    /// Runs embeddings, backbone and heads once; returns logits (8, S, 1025).
    fn step(&self, ids: &[i64], mask: &[bool], seq: usize) -> Result<Vec<f32>, String> {
        let e = |e: ort::Error| e.to_string();
        let embeds = {
            let mut s = self.embeddings.lock().unwrap_or_else(|p| p.into_inner());
            let out = s
                .run(ort::inputs![
                    "input_ids" => Tensor::from_array(([1usize, CODEBOOKS, seq], ids.to_vec())).map_err(e)?,
                    "audio_mask" => Tensor::from_array(([1usize, seq], mask.to_vec())).map_err(e)?,
                ])
                .map_err(e)?;
            super::floats(&out["inputs_embeds"])?
        };
        let hidden = {
            let mut s = self.llm.lock().unwrap_or_else(|p| p.into_inner());
            let names: Vec<String> = s.inputs().iter().map(|i| i.name().to_string()).collect();
            let width = embeds.len() / seq;
            let mut feed: Vec<(String, DynValue)> = vec![
                ("inputs_embeds".into(), Tensor::from_array(([1usize, seq, width], embeds)).map_err(e)?.into_dyn()),
            ];
            for n in &names {
                if n == "attention_mask" {
                    feed.push((n.clone(), Tensor::from_array(([1usize, seq], vec![1i64; seq])).map_err(e)?.into_dyn()));
                } else if n == "position_ids" {
                    let pos: Vec<i64> = (0..seq as i64).collect();
                    feed.push((n.clone(), Tensor::from_array(([1usize, seq], pos)).map_err(e)?.into_dyn()));
                } else if n.contains("past") {
                    // No cache: the whole sequence is run every step.
                    feed.push((n.clone(), Tensor::from_array(([1usize, 8, 0, 128], Vec::<f32>::new())).map_err(e)?.into_dyn()));
                }
            }
            let out = s.run(feed).map_err(e)?;
            super::floats(&out["hidden_states"])?
        };
        let mut s = self.heads.lock().unwrap_or_else(|p| p.into_inner());
        let width = hidden.len() / seq;
        let out = s
            .run(ort::inputs!["hidden_states" => Tensor::from_array(([1usize, seq, width], hidden)).map_err(e)?])
            .map_err(e)?;
        super::floats(&out["logits"])
    }

    /// Confidence-ordered iterative unmasking; returns codes (8, frames).
    fn unmask(&self, text: &[i64], frames: usize) -> Result<Vec<i64>, String> {
        let t_text = text.len();
        let seq = t_text + frames;
        let mut ids = vec![MASK_ID; CODEBOOKS * seq];
        for cb in 0..CODEBOOKS {
            ids[cb * seq..cb * seq + t_text].copy_from_slice(text);
        }
        let mask: Vec<bool> = (0..seq).map(|i| i >= t_text).collect();
        let wsum: f32 = CB_WEIGHTS.iter().sum();
        let at = |cb: usize, pos: usize| cb * seq + t_text + pos;

        for step in 0..STEPS {
            let masked: Vec<usize> = (0..frames).filter(|&p| ids[at(0, p)] == MASK_ID).collect();
            if masked.is_empty() {
                break;
            }
            let logits = self.step(&ids, &mask, seq)?;
            let row = |cb: usize, pos: usize| {
                let start = (cb * seq + t_text + pos) * HEAD_VOCAB;
                &logits[start..start + REAL_CODES]
            };
            let mut scored: Vec<(usize, f32)> = masked
                .iter()
                .map(|&p| {
                    let conf: f32 = (0..CODEBOOKS).map(|cb| max_prob(row(cb, p)) * CB_WEIGHTS[cb] / wsum).sum();
                    (p, conf)
                })
                .collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            let take = masked.len().div_ceil(STEPS - step).max(1);
            for &(p, _) in scored.iter().take(take) {
                for cb in 0..CODEBOOKS {
                    ids[at(cb, p)] = argmax(row(cb, p)) as i64;
                }
            }
        }
        let mut codes = Vec::with_capacity(CODEBOOKS * frames);
        for cb in 0..CODEBOOKS {
            codes.extend((0..frames).map(|p| ids[at(cb, p)].min(REAL_CODES as i64 - 1)));
        }
        Ok(codes)
    }

    fn decode(&self, codes: &[i64], frames: usize) -> Result<Vec<f32>, String> {
        let e = |e: ort::Error| e.to_string();
        let mut s = self.decoder.lock().unwrap_or_else(|p| p.into_inner());
        let out = s
            .run(ort::inputs!["codes" => Tensor::from_array(([CODEBOOKS, 1usize, frames], codes.to_vec())).map_err(e)?])
            .map_err(e)?;
        super::floats(&out["waveform_24k"])
    }
}

/// Rough speech length: about 14 characters a second for alphabetic text,
/// 4 a second for Chinese, Japanese and Korean, plus a short tail.
fn estimate_frames(text: &str) -> usize {
    let secs: f32 = text
        .chars()
        .map(|c| match c as u32 {
            0x3040..=0x30FF | 0x3400..=0x9FFF | 0xAC00..=0xD7AF => 0.25,
            _ if c.is_whitespace() => 0.03,
            _ => 1.0 / 14.0,
        })
        .sum::<f32>()
        + 0.4;
    ((secs * FRAMES_PER_SEC).ceil() as usize).clamp(25, MAX_FRAMES)
}

fn argmax(row: &[f32]) -> usize {
    row.iter().enumerate().fold((0, f32::MIN), |b, (i, &v)| if v > b.1 { (i, v) } else { b }).0
}

/// Largest softmax probability of a row.
fn max_prob(row: &[f32]) -> f32 {
    let m = row.iter().copied().fold(f32::MIN, f32::max);
    let sum: f32 = row.iter().map(|v| (v - m).exp()).sum();
    1.0 / sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_estimate_scales_with_text() {
        let short = estimate_frames("Hi");
        let long = estimate_frames("This is a much longer sentence to read aloud.");
        assert!(short >= 25 && long > short);
        assert!(estimate_frames("你好世界") > estimate_frames("abcd"));
        assert_eq!(estimate_frames(&"a".repeat(10_000)), MAX_FRAMES);
    }

    #[test]
    fn max_prob_is_softmax_peak() {
        assert!((max_prob(&[0.0, 0.0]) - 0.5).abs() < 1e-6);
        assert!(max_prob(&[10.0, 0.0, 0.0]) > 0.99);
    }
}
