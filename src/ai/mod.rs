//! On-device AI, all through ONNX Runtime: the planner ([`llm`]), speech
//! recognition ([`asr`]) and speech ([`tts`]). The models come in one pack
//! ([`pack`]) that is downloaded the first time any of them is needed.

pub mod asr;
pub mod llm;
pub mod pack;
pub mod tts;

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use ort::environment::Environment;
use ort::session::Session;

/// One ONNX Runtime environment for every model in the app.
fn environment() -> Result<&'static Environment, String> {
    static ENV: OnceLock<Environment> = OnceLock::new();
    if let Some(env) = ENV.get() {
        return Ok(env);
    }
    let env = ort::init().with_name("thulli").build().map_err(|e| e.to_string())?;
    Ok(ENV.get_or_init(|| env))
}

/// Reads a model output as `f32`, converting from `f16` when the model was
/// exported in half precision (OmniVoice's audio decoder is).
pub(crate) fn floats(value: &ort::value::DynValue) -> Result<Vec<f32>, String> {
    if let Ok((_, v)) = value.try_extract_tensor::<f32>() {
        return Ok(v.to_vec());
    }
    let (_, v) = value.try_extract_tensor::<half::f16>().map_err(|e| e.to_string())?;
    Ok(v.iter().map(|x| x.to_f32()).collect())
}

/// Opens an ONNX model file on the CPU.
pub(crate) fn open_session(path: &Path) -> Result<Session, String> {
    Session::builder(environment()?)
        .and_then(|mut b| b.commit_from_file(path))
        .map_err(|e| format!("{}: {e}", path.file_name().and_then(|f| f.to_str()).unwrap_or("model")))
}

/// Loaded models, kept for the life of the app once first used.
#[derive(Default)]
pub struct Models {
    llm: Mutex<Option<Arc<llm::Llm>>>,
    asr: Mutex<Option<Arc<asr::Asr>>>,
    tts: Mutex<Option<Arc<tts::Tts>>>,
}

macro_rules! getter {
    ($name:ident, $ty:ty, $dir:literal, $load:expr) => {
        /// Downloads the pack if needed, then loads this model once.
        pub fn $name(&self, on_progress: &mut dyn FnMut(pack::PackProgress)) -> Result<Arc<$ty>, String> {
            let mut slot = self.$name.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(m) = slot.as_ref() {
                return Ok(m.clone());
            }
            let dir = pack::ensure(on_progress)?.join($dir);
            let model = Arc::new($load(&dir)?);
            *slot = Some(model.clone());
            Ok(model)
        }
    };
}

impl Models {
    getter!(llm, llm::Llm, "llm", |d: &std::path::Path| llm::Llm::load(d).map_err(|e| e.user_message()));
    getter!(asr, asr::Asr, "asr", asr::Asr::load);
    getter!(tts, tts::Tts, "tts", tts::Tts::load);

    /// Forgets loaded models, for example after the files were removed.
    pub fn unload(&self) {
        *self.llm.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *self.asr.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *self.tts.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}
