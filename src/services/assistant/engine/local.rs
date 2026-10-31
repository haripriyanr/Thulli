//! Real engine: runs a user-chosen GGUF file on this machine with `mistralrs`.
//!
//! The model is loaded from a local directory and file name, so the library
//! never contacts a model hub. Nothing is downloaded and no network call is
//! made by this module.

use std::path::Path;

use mistralrs::{
    ChatCompletionChunkResponse, ChunkChoice, Delta, GgufModelBuilder, Model, RequestBuilder, Response,
    TextMessageRole,
};
use tokio::runtime::Runtime;

use super::{ChatEngine, EngineError, EngineMessage, EngineRole};

/// Longest reply, in tokens.
const MAX_REPLY_TOKENS: usize = 1024;
/// Low temperature keeps the JSON action block well formed.
const TEMPERATURE: f64 = 0.4;

pub struct LocalGgufEngine {
    runtime: Runtime,
    model: Model,
}

impl LocalGgufEngine {
    /// Loads the model. Slow; call from a worker thread.
    pub fn load(path: &Path) -> Result<Self, EngineError> {
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| EngineError::BadModelFile("the file has no folder".into()))?;
        let file = path
            .file_name()
            .and_then(|f| f.to_str())
            .ok_or_else(|| EngineError::BadModelFile("the file name is not valid text".into()))?;
        let dir = dir
            .to_str()
            .ok_or_else(|| EngineError::BadModelFile("the folder name is not valid text".into()))?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| EngineError::Failed(e.to_string()))?;
        let model = runtime
            .block_on(GgufModelBuilder::new(dir, vec![file]).build())
            .map_err(|e| EngineError::Failed(e.to_string()))?;
        Ok(Self { runtime, model })
    }
}

fn role(role: EngineRole) -> TextMessageRole {
    match role {
        EngineRole::System => TextMessageRole::System,
        EngineRole::User => TextMessageRole::User,
        EngineRole::Assistant => TextMessageRole::Assistant,
    }
}

impl ChatEngine for LocalGgufEngine {
    fn generate(
        &self,
        messages: &[EngineMessage],
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<String, EngineError> {
        let mut request = RequestBuilder::new()
            .set_sampler_temperature(TEMPERATURE)
            .set_sampler_max_len(MAX_REPLY_TOKENS);
        for m in messages {
            request = request.add_message(role(m.role), &m.content);
        }

        self.runtime.block_on(async {
            let mut stream = self
                .model
                .stream_chat_request(request)
                .await
                .map_err(|e| EngineError::Failed(e.to_string()))?;
            let mut out = String::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Response::Chunk(ChatCompletionChunkResponse { choices, .. }) => {
                        if let Some(ChunkChoice { delta: Delta { content: Some(text), .. }, .. }) = choices.first() {
                            out.push_str(text);
                            if !on_token(text) {
                                return Err(EngineError::Cancelled);
                            }
                        }
                    }
                    Response::InternalError(e) | Response::ValidationError(e) => {
                        return Err(EngineError::Failed(e.to_string()));
                    }
                    Response::ModelError(msg, _) => return Err(EngineError::Failed(msg)),
                    _ => {}
                }
            }
            Ok(out)
        })
    }
}
