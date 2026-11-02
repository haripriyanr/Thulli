//! The AI side of [`DataHub`]: the API key, the Plan with AI panel and the
//! Sort screen. Model calls run on a worker thread; the reply comes back to
//! the UI thread through `AppData.ai-reply`. Nothing the model proposes is
//! applied until the user agrees.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use chrono::Local;
use slint::ComponentHandle;
use thulli_core::models::MatrixQuadrant;
use thulli_core::services::assistant::{EngineMessage, EngineRole, RemoteEngine};
use thulli_core::services::pomodoro::PomodoroPhase;
use thulli_core::services::tasks::matrix;

use super::DataHub;
use crate::convert::model;
use crate::secrets;
use crate::spaces;
use crate::{AppData, MainWindow};

/// Where the Sort screen is: the unsorted tasks it walks through and the
/// AI's suggestions for them.
#[derive(Default)]
pub struct Triage {
    queue: Vec<String>,
    index: usize,
    decided: usize,
    suggestions: HashMap<String, MatrixQuadrant>,
}

impl DataHub {
    /// Points the assistant at OpenRouter when a key is saved.
    pub(super) fn connect_ai(&self) {
        match secrets::load(secrets::AI_KEY) {
            Some(key) => {
                let model = self.prefs.snapshot().ai_model;
                self.assistant.set_engine(Arc::new(RemoteEngine::openrouter(key, model)));
            }
            None => self.assistant.clear_engine(),
        }
    }

    pub(super) fn refresh_ai(&self, window: &MainWindow) {
        let data = window.global::<AppData>();
        data.set_ai_key_saved(secrets::load(secrets::AI_KEY).is_some());
        data.set_ai_model(self.prefs.snapshot().ai_model.into());
        self.refresh_triage(window);
    }

    pub(super) fn refresh_triage(&self, window: &MainWindow) {
        let data = window.global::<AppData>();
        let triage = self.triage.borrow();
        data.set_triage_total(triage.queue.len() as i32);
        data.set_triage_decided(triage.decided as i32);
        data.set_triage_position((triage.index + 1).min(triage.queue.len()) as i32);
        let card = triage
            .queue
            .get(triage.index)
            .and_then(|id| self.handle.read(|s| s.task(id).map(|t| spaces::card(s, t))));
        data.set_triage_has_task(card.is_some());
        if let Some(card) = card {
            data.set_triage_task(card);
        }
        let suggestion = triage
            .queue
            .get(triage.index)
            .and_then(|id| triage.suggestions.get(id))
            .map(|q| q.as_str())
            .unwrap_or("");
        data.set_triage_suggestion(suggestion.into());
    }

    /// Sends `prompt` on a worker thread. The reply arrives as `ai-reply`.
    fn run_ai(&self, window: &MainWindow, kind: &str, prompt: Vec<EngineMessage>) {
        let data = window.global::<AppData>();
        let engine = match self.assistant.engine_for_request() {
            Ok(engine) => engine,
            Err(_) => {
                data.set_ai_status("Add an OpenRouter API key in Settings to use AI.".into());
                return;
            }
        };
        data.set_ai_busy(true);
        data.set_ai_status("".into());
        let weak = window.as_weak();
        let kind = kind.to_string();
        std::thread::spawn(move || {
            let (ok, text) = match engine.generate(&prompt, &mut |_| true) {
                Ok(text) => (true, text),
                Err(e) => (false, e.user_message()),
            };
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.global::<AppData>().invoke_ai_reply(kind.into(), ok, text.into());
            });
        });
    }

    /// Starts a focus round on `task_id` with the session linked to it.
    pub(super) fn launch_focus(&self, window: &MainWindow, task_id: &str) {
        self.tasks.start_focus(task_id);
        *self.task_choice.borrow_mut() = Some(task_id.to_string());
        *self.mode.borrow_mut() = "focus".to_string();
        self.switch_phase(window, PomodoroPhase::Focus);
        self.pomodoro.borrow_mut().start();
        self.refresh_all(window);
    }

    fn handle_reply(&self, window: &MainWindow, kind: &str, ok: bool, text: &str) {
        let data = window.global::<AppData>();
        data.set_ai_busy(false);
        if !ok {
            data.set_ai_status(text.into());
            return;
        }
        match kind {
            "test" => data.set_ai_status("Connected. The model answered.".into()),
            "plan" => {
                let lines = self.assistant.accept_reply(text);
                if lines.is_empty() {
                    data.set_ai_status("The AI had no changes to suggest. Try saying more.".into());
                }
                data.set_ai_proposals(model(lines.into_iter().map(Into::into).collect()));
            }
            "sort" => {
                self.assistant.accept_reply(text);
                let suggested = self.assistant.pending_quadrants();
                // Suggestions are only shown; each still needs the user's choice.
                self.assistant.discard_pending();
                let count = suggested.len();
                self.triage.borrow_mut().suggestions.extend(suggested);
                data.set_ai_status(match count {
                    0 => "The AI could not suggest quadrants for these tasks.".into(),
                    n => format!("The AI suggested quadrants for {n} {}.", if n == 1 { "task" } else { "tasks" }).into(),
                });
                self.refresh_triage(window);
            }
            _ => {}
        }
    }

    pub(super) fn wire_ai(self: &Rc<Self>, window: &MainWindow) {
        let data = window.global::<AppData>();
        data.on_mic_toggle(crate::voice::toggle_voice_typing);
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_reply(move |kind, ok, text| {
                if let Some(w) = weak.upgrade() {
                    hub.handle_reply(&w, &kind, ok, &text);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_key_save(move |key| {
                let Some(w) = weak.upgrade() else { return };
                let key = key.trim();
                let status = if key.is_empty() {
                    "Paste a key first."
                } else if secrets::save(secrets::AI_KEY, key) {
                    hub.connect_ai();
                    "Key saved in Windows Credential Manager."
                } else {
                    "The key could not be saved on this device."
                };
                w.global::<AppData>().set_ai_status(status.into());
                hub.refresh_ai(&w);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_key_remove(move || {
                let Some(w) = weak.upgrade() else { return };
                secrets::remove(secrets::AI_KEY);
                hub.connect_ai();
                w.global::<AppData>().set_ai_status("Key removed.".into());
                hub.refresh_ai(&w);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_model_set(move |id| {
                let Some(w) = weak.upgrade() else { return };
                let status = match hub.prefs.set_text("ai_model", &id) {
                    Ok(()) => "Model saved.",
                    Err(_) => "Use a model id such as openai/gpt-4o-mini.",
                };
                hub.connect_ai();
                w.global::<AppData>().set_ai_status(status.into());
                hub.refresh_ai(&w);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_test(move || {
                let Some(w) = weak.upgrade() else { return };
                let prompt = vec![
                    EngineMessage::new(EngineRole::System, "Reply with the single word OK."),
                    EngineMessage::new(EngineRole::User, "ping"),
                ];
                hub.run_ai(&w, "test", prompt);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_plan(move |request, start_now| {
                let Some(w) = weak.upgrade() else { return };
                let mut request = request.to_string();
                if start_now {
                    request.push_str("\nStart a focus round on the first task now.");
                }
                match hub.assistant.plan_prompt(&request, Local::now()) {
                    Ok(prompt) => {
                        w.global::<AppData>().set_ai_proposals(model(Vec::new()));
                        hub.run_ai(&w, "plan", prompt);
                    }
                    Err(why) => w.global::<AppData>().set_ai_status(why.into()),
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_apply(move || {
                let Some(w) = weak.upgrade() else { return };
                let report = hub.assistant.apply_pending(None);
                let data = w.global::<AppData>();
                data.set_ai_proposals(model(Vec::new()));
                data.set_ai_status(report.summary().into());
                data.set_ai_focus_started(report.focus_task.is_some());
                match report.focus_task {
                    Some(id) => hub.launch_focus(&w, &id),
                    None => hub.refresh_all(&w),
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_discard(move || {
                hub.assistant.discard_pending();
                if let Some(w) = weak.upgrade() {
                    w.global::<AppData>().set_ai_proposals(model(Vec::new()));
                }
            });
        }

        // Sort screen
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_triage_start(move || {
                let Some(w) = weak.upgrade() else { return };
                let space = hub.space.borrow().clone();
                let queue = hub.handle.read(|s| matrix::unsorted_queue(&matrix::build(s, &space)));
                *hub.triage.borrow_mut() = Triage { queue, ..Triage::default() };
                w.global::<AppData>().set_triage_open(true);
                hub.refresh_triage(&w);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_triage_decide(move |quadrant| {
                let Some(w) = weak.upgrade() else { return };
                let id = {
                    let t = hub.triage.borrow();
                    t.queue.get(t.index).cloned()
                };
                if let (Some(id), Some(q)) = (id, MatrixQuadrant::parse(&quadrant)) {
                    hub.tasks.set_quadrant(&hub.history, &id, Some(q));
                    let mut t = hub.triage.borrow_mut();
                    t.decided += 1;
                    t.index += 1;
                }
                hub.refresh_all(&w);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_triage_skip(move || {
                {
                    let mut t = hub.triage.borrow_mut();
                    t.index = (t.index + 1).min(t.queue.len());
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_triage(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_triage_back(move || {
                {
                    let mut t = hub.triage.borrow_mut();
                    t.index = t.index.saturating_sub(1);
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_triage(&w);
                }
            });
        }
        {
            let weak = window.as_weak();
            data.on_triage_close(move || {
                if let Some(w) = weak.upgrade() {
                    w.global::<AppData>().set_triage_open(false);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_triage_ask_ai(move || {
                let Some(w) = weak.upgrade() else { return };
                let prompt = hub.assistant.sort_prompt(Local::now());
                hub.run_ai(&w, "sort", prompt);
            });
        }
    }
}
