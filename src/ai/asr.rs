//! Speech recognition with Meta's Omnilingual ASR (CTC, 300M, int8 ONNX).
//!
//! The graph takes the raw 16 kHz waveform and returns per-frame logits over
//! one shared vocabulary for every language; greedy CTC decoding turns them
//! into text. Nothing selects a language: it transcribes what it hears.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use ort::session::Session;
use ort::value::Tensor;

/// Sample rate the model expects.
pub const SAMPLE_RATE: u32 = 16_000;
/// The CTC blank (`<s>`) is token 0.
const BLANK: usize = 0;

pub struct Asr {
    session: Mutex<Session>,
    vocab: HashMap<usize, String>,
}

impl Asr {
    /// Loads `model.int8.onnx` and `tokens.txt` from `dir`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let session = super::open_session(&dir.join("model.int8.onnx"))
            .map_err(|e| format!("could not load speech recognition: {e}"))?;
        let text = std::fs::read_to_string(dir.join("tokens.txt")).map_err(|e| e.to_string())?;
        Ok(Self { session: Mutex::new(session), vocab: parse_tokens(&text) })
    }

    /// Transcribes mono 16 kHz samples in [-1, 1].
    pub fn transcribe(&self, samples: &[f32]) -> Result<String, String> {
        if samples.len() < SAMPLE_RATE as usize / 4 {
            return Ok(String::new());
        }
        let input = Tensor::from_array(([1usize, samples.len()], samples.to_vec())).map_err(|e| e.to_string())?;
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let outputs = session.run(ort::inputs!["x" => input]).map_err(|e| e.to_string())?;
        let (shape, logits) = outputs["logits"].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        let frames = shape[1] as usize;
        let vocab = shape[2] as usize;
        let ids = (0..frames).map(|t| argmax(&logits[t * vocab..(t + 1) * vocab]));
        Ok(ctc_text(ids, &self.vocab))
    }
}

/// `tokens.txt` lines are `<token> <id>`; a token may itself be a space.
fn parse_tokens(text: &str) -> HashMap<usize, String> {
    text.lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let (tok, id) = l.rsplit_once(' ')?;
            Some((id.trim().parse().ok()?, tok.to_string()))
        })
        .collect()
}

fn argmax(row: &[f32]) -> usize {
    row.iter().enumerate().fold((0, f32::MIN), |b, (i, &v)| if v > b.1 { (i, v) } else { b }).0
}

/// Greedy CTC: merge repeats, drop blanks and special tokens, join as is.
fn ctc_text(ids: impl Iterator<Item = usize>, vocab: &HashMap<usize, String>) -> String {
    let mut out = String::new();
    let mut prev = usize::MAX;
    for id in ids {
        if id != prev && id != BLANK {
            if let Some(tok) = vocab.get(&id) {
                if !(tok.starts_with('<') && tok.ends_with('>')) {
                    out.push_str(tok);
                }
            }
        }
        prev = id;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctc_merges_repeats_and_drops_blanks() {
        let vocab = parse_tokens("<s> 0\nh 1\ni 2\n  3\n");
        assert_eq!(vocab[&3], " ");
        let text = ctc_text([1, 1, 0, 2, 2, 3, 0, 1, 0, 1].into_iter(), &vocab);
        assert_eq!(text, "hi hh");
    }
}
