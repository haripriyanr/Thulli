//! Chat engine abstraction for the on-device assistant.
//!
//! The assistant talks to a [`ChatEngine`]. The real engine (behind the
//! `local-model` feature) runs a GGUF file chosen by the user, entirely on this
//! machine. [`MockEngine`] serves scripted replies for tests.

use std::collections::VecDeque;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[cfg(feature = "local-model")]
mod local;
mod remote;

pub use remote::{RemoteEngine, DEFAULT_MODEL, OPENROUTER_URL};

/// Who wrote a message handed to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineRole {
    System,
    User,
    Assistant,
}

/// One message in the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineMessage {
    pub role: EngineRole,
    pub content: String,
}

impl EngineMessage {
    pub fn new(role: EngineRole, content: impl Into<String>) -> Self {
        Self { role, content: content.into() }
    }
}

/// Why a reply could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// No model file has been chosen.
    NotChosen,
    /// The chosen file is missing or not usable.
    BadModelFile(String),
    /// This build has no local model support.
    Unsupported,
    /// The model failed while loading or answering.
    Failed(String),
    /// The caller stopped generation.
    Cancelled,
}

impl EngineError {
    /// Plain wording suitable for showing to the user.
    pub fn user_message(&self) -> String {
        match self {
            EngineError::NotChosen => {
                "Choose a model file in Preferences to turn the assistant on.".to_string()
            }
            EngineError::BadModelFile(why) => format!("The model file could not be used: {why}"),
            EngineError::Unsupported => {
                "This version of Thulli was built without on-device assistant support.".to_string()
            }
            EngineError::Failed(why) => format!("The assistant could not answer: {why}"),
            EngineError::Cancelled => "Stopped.".to_string(),
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.user_message())
    }
}

impl std::error::Error for EngineError {}

/// A blocking chat model. Call it from a worker thread, never the UI thread.
pub trait ChatEngine: Send + Sync {
    /// Produces a reply to `messages`. `on_token` receives each new piece of
    /// text as it is produced; returning `false` from it stops generation.
    /// The full reply is returned.
    fn generate(
        &self,
        messages: &[EngineMessage],
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<String, EngineError>;
}

/// State of the model chosen in preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStatus {
    NotChosen,
    Missing,
    NotGguf,
    /// Chosen and present, but this build cannot run it.
    Unsupported,
    Ready,
}

impl ModelStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, ModelStatus::Ready)
    }

    /// Message shown in place of the chat input when the model is not ready.
    pub fn notice(&self) -> &'static str {
        match self {
            ModelStatus::NotChosen => {
                "No model chosen yet. Pick a model file in Preferences and the assistant will run on this device. Nothing is ever downloaded or sent anywhere."
            }
            ModelStatus::Missing => {
                "The model file you chose could not be found. Pick it again in Preferences."
            }
            ModelStatus::NotGguf => "The chosen file is not a model file. Pick a file ending in .gguf in Preferences.",
            ModelStatus::Unsupported => {
                "This version of Thulli was built without on-device assistant support."
            }
            ModelStatus::Ready => "",
        }
    }
}

/// True when this build can run a real model.
pub const fn local_model_supported() -> bool {
    cfg!(feature = "local-model")
}

/// Checks the model path from preferences without loading it.
pub fn model_status(path: &str) -> ModelStatus {
    let path = path.trim();
    if path.is_empty() {
        return ModelStatus::NotChosen;
    }
    let p = Path::new(path);
    if !p.is_file() {
        return ModelStatus::Missing;
    }
    let is_gguf = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gguf"));
    if !is_gguf {
        return ModelStatus::NotGguf;
    }
    if !local_model_supported() {
        return ModelStatus::Unsupported;
    }
    ModelStatus::Ready
}

/// Loads the real engine for a model file. Blocks while the model loads.
pub fn load_local_engine(path: &str) -> Result<Arc<dyn ChatEngine>, EngineError> {
    match model_status(path) {
        ModelStatus::NotChosen => Err(EngineError::NotChosen),
        ModelStatus::Missing => Err(EngineError::BadModelFile("the file was not found".into())),
        ModelStatus::NotGguf => Err(EngineError::BadModelFile("it is not a .gguf file".into())),
        ModelStatus::Unsupported => Err(EngineError::Unsupported),
        ModelStatus::Ready => load_ready(path),
    }
}

#[cfg(feature = "local-model")]
fn load_ready(path: &str) -> Result<Arc<dyn ChatEngine>, EngineError> {
    Ok(Arc::new(local::LocalGgufEngine::load(Path::new(path.trim()))?))
}

#[cfg(not(feature = "local-model"))]
fn load_ready(_path: &str) -> Result<Arc<dyn ChatEngine>, EngineError> {
    Err(EngineError::Unsupported)
}

/// Test engine returning scripted replies, one per call, streamed in small
/// chunks. Records every prompt it receives.
pub struct MockEngine {
    replies: Mutex<VecDeque<Result<String, EngineError>>>,
    seen: Mutex<Vec<Vec<EngineMessage>>>,
    chunk_chars: usize,
}

impl MockEngine {
    pub fn new(replies: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().map(|r| Ok(r.into())).collect()),
            seen: Mutex::new(Vec::new()),
            chunk_chars: 4,
        }
    }

    /// An engine whose next call fails.
    pub fn failing(error: EngineError) -> Self {
        Self { replies: Mutex::new(VecDeque::from([Err(error)])), seen: Mutex::new(Vec::new()), chunk_chars: 4 }
    }

    /// Prompts received so far, oldest first.
    pub fn prompts(&self) -> Vec<Vec<EngineMessage>> {
        self.seen.lock().unwrap().clone()
    }
}

impl ChatEngine for MockEngine {
    fn generate(
        &self,
        messages: &[EngineMessage],
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<String, EngineError> {
        self.seen.lock().unwrap().push(messages.to_vec());
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(EngineError::Failed("no scripted reply".into())))?;
        let mut produced = String::new();
        let chars: Vec<char> = reply.chars().collect();
        for piece in chars.chunks(self.chunk_chars) {
            let piece: String = piece.iter().collect();
            produced.push_str(&piece);
            if !on_token(&piece) {
                return Err(EngineError::Cancelled);
            }
        }
        Ok(produced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reflects_the_chosen_path() {
        assert_eq!(model_status(""), ModelStatus::NotChosen);
        assert_eq!(model_status("   "), ModelStatus::NotChosen);
        assert_eq!(model_status("Z:/definitely/not/here.gguf"), ModelStatus::Missing);
        let dir = std::env::temp_dir().join("thulli_engine_status_test");
        std::fs::create_dir_all(&dir).unwrap();
        let txt = dir.join("model.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert_eq!(model_status(txt.to_str().unwrap()), ModelStatus::NotGguf);
        let gguf = dir.join("model.gguf");
        std::fs::write(&gguf, b"x").unwrap();
        let expected = if local_model_supported() { ModelStatus::Ready } else { ModelStatus::Unsupported };
        assert_eq!(model_status(gguf.to_str().unwrap()), expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mock_streams_chunks_and_can_be_stopped() {
        let engine = MockEngine::new(["hello world"]);
        let mut got = String::new();
        let out = engine
            .generate(&[EngineMessage::new(EngineRole::User, "hi")], &mut |t| {
                got.push_str(t);
                true
            })
            .unwrap();
        assert_eq!(out, "hello world");
        assert_eq!(got, "hello world");
        assert_eq!(engine.prompts().len(), 1);

        let engine = MockEngine::new(["hello world"]);
        let r = engine.generate(&[], &mut |_| false);
        assert_eq!(r, Err(EngineError::Cancelled));
    }
}
