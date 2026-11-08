//! The AI side of [`DataHub`]: the on-device models, the mic, reading aloud,
//! the Plan with AI panel and the Sort screen. Model calls run on worker
//! threads; results come back to the UI thread through `AppData.ai-reply` or
//! `upgrade_in_event_loop`. Nothing the model proposes is applied until the
//! user agrees.

use std::collections::HashMap;
use std::rc::Rc;

use chrono::Local;
use slint::ComponentHandle;
use thulli_core::models::MatrixQuadrant;
use thulli_core::ai::{asr, pack, tts};
use thulli_core::services::assistant::{ChatEngine, EngineMessage};
use thulli_core::services::pomodoro::PomodoroPhase;
use thulli_core::services::tasks::matrix;

use super::DataHub;
use crate::convert::model;
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
    pub(super) fn refresh_ai(&self, window: &MainWindow) {
        window.global::<AppData>().set_ai_models_ready(pack::is_ready());        self.refresh_triage(window);
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

    /// Sends `prompt` to the planner on a worker thread, downloading the
    /// models first when needed. The reply arrives as `ai-reply`.
    fn run_ai(&self, window: &MainWindow, kind: &str, prompt: Vec<EngineMessage>) {
        let data = window.global::<AppData>();
        data.set_ai_busy(true);
        data.set_ai_status("".into());
        let weak = window.as_weak();
        let kind = kind.to_string();
        let models = self.models.clone();
        std::thread::spawn(move || {
            let status = |text: String| {
                let _ = weak.upgrade_in_event_loop(move |w| w.global::<AppData>().set_ai_status(text.into()));
            };
            let llm = models.llm(&mut |p| status(p.describe()));
            let (ok, text) = match llm {
                Err(why) => (false, format!("The AI could not start: {why}")),
                Ok(llm) => {
                    status("Writing the plan...".into());
                    let reply = llm.generate(&prompt, &mut |_| true);
                    match reply {
                        Ok(text) => (true, text),
                        Err(e) => (false, e.user_message()),
                    }
                }
            };
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.global::<AppData>().set_ai_models_ready(pack::is_ready());
                w.global::<AppData>().invoke_ai_reply(kind.into(), ok, text.into());
            });
        });
    }

    /// Turns the mic for `owner` on, or off and transcribes what was said.
    fn toggle_mic(&self, window: &MainWindow, owner: &str) {
        let data = window.global::<AppData>();
        let open = self.recorder.borrow_mut().take();
        if let Some((was, recorder)) = open {
            data.set_mic_listening("".into());
            let samples = recorder.finish(asr::SAMPLE_RATE);
            if was != owner {
                // Another field's mic was on; switch to this one.
                return self.toggle_mic(window, owner);
            }
            data.set_mic_busy(true);
            let weak = window.as_weak();
            let models = self.models.clone();
            std::thread::spawn(move || {
                let status = |text: String| {
                    let _ = weak.upgrade_in_event_loop(move |w| w.global::<AppData>().set_ai_status(text.into()));
                };
                let text = models
                    .asr(&mut |p| status(p.describe()))
                    .and_then(|m| m.transcribe(&samples));
                let _ = weak.upgrade_in_event_loop(move |w| {
                    let data = w.global::<AppData>();
                    data.set_mic_busy(false);
                    data.set_ai_models_ready(pack::is_ready());
                    match text {
                        Ok(t) if !t.is_empty() => {
                            data.set_ai_status("".into());
                            data.set_mic_owner(was.into());
                            data.set_mic_text(t.into());
                            data.set_mic_seq(data.get_mic_seq() + 1);
                        }
                        Ok(_) => data.set_ai_status("Nothing was heard. Try again a little closer to the mic.".into()),
                        Err(why) => data.set_ai_status(format!("Speech recognition failed: {why}").into()),
                    }
                });
            });
            return;
        }
        match crate::voice::Recorder::start() {
            Ok(rec) => {
                *self.recorder.borrow_mut() = Some((owner.to_string(), rec));
                data.set_mic_listening(owner.into());
            }
            Err(why) => data.set_ai_status(why.into()),
        }
    }

    /// Reads `text` aloud on a worker thread.
    fn speak(&self, window: &MainWindow, text: String) {
        let weak = window.as_weak();
        let models = self.models.clone();
        std::thread::spawn(move || {
            let status = |text: String| {
                let _ = weak.upgrade_in_event_loop(move |w| w.global::<AppData>().set_ai_status(text.into()));
            };
            let result = models
                .tts(&mut |p| status(p.describe()))
                .and_then(|m| m.speak(&text))
                .and_then(|wave| crate::voice::play(wave, tts::SAMPLE_RATE));
            if let Err(why) = result {
                status(format!("Reading aloud failed: {why}"));
            }
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

    /// Stops the timer and logs the time so far to the chosen task.
    pub(super) fn stop_focus(&self, window: &MainWindow) {
        let (phase, elapsed) = {
            let timer = self.pomodoro.borrow();
            (timer.phase(), timer.elapsed_in_phase())
        };
        self.pomodoro.borrow_mut().reset();
        self.record_time(phase, elapsed, false);
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
            "plan" => {
                let (lines, rejected) = self.assistant.accept_reply_with_reasons(text);
                if lines.is_empty() {
                    // Say why, so a refused reply is not mistaken for an empty one.
                    let status = match rejected.first() {
                        Some(why) => format!("The AI's changes could not be used ({why}). Try again or rephrase."),
                        None => "The AI had no changes to suggest. Try saying more.".to_string(),
                    };
                    data.set_ai_status(status.into());
                } else if !rejected.is_empty() {
                    data.set_ai_status(format!("{} suggested change(s) were left out because they were invalid.", rejected.len()).into());
                } else {
                    data.set_ai_status("".into());
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
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_mic_toggle(move |owner| {
                if let Some(w) = weak.upgrade() {
                    hub.toggle_mic(&w, &owner);
                }
            });
        }
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
            data.on_ai_models_download(move || {
                let Some(w) = weak.upgrade() else { return };
                w.global::<AppData>().set_ai_busy(true);
                let weak = w.as_weak();
                let models = hub.models.clone();
                std::thread::spawn(move || {
                    let status = |text: String| {
                        let _ = weak.upgrade_in_event_loop(move |w| w.global::<AppData>().set_ai_status(text.into()));
                    };
                    let done = pack::ensure(&mut |p| status(p.describe()));
                    let _ = models; // kept so a later load reuses this download
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        let data = w.global::<AppData>();
                        data.set_ai_busy(false);
                        data.set_ai_models_ready(pack::is_ready());
                        data.set_ai_status(match done {
                            Ok(_) => "".into(),
                            Err(why) => format!("The download did not finish: {why}").into(),
                        });
                    });
                });
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_models_refresh(move || {
                let Some(w) = weak.upgrade() else { return };
                hub.refresh_ai(&w);
                // The chip and buttons already show the state.
                w.global::<AppData>().set_ai_status("".into());
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_models_remove(move || {
                let Some(w) = weak.upgrade() else { return };
                hub.models.unload();
                let status = match pack::remove() {
                    Ok(()) => "AI models removed.".to_string(),
                    Err(why) => why,
                };
                w.global::<AppData>().set_ai_status(status.into());
                hub.refresh_ai(&w);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ai_speak(move |text| {
                let Some(w) = weak.upgrade() else { return };
                // Empty text reads the proposed changes.
                let text = if text.is_empty() {
                    let data = w.global::<AppData>();
                    use slint::Model;
                    data.get_ai_proposals().iter().map(|l| l.to_string()).collect::<Vec<_>>().join(". ")
                } else {
                    text.to_string()
                };
                hub.speak(&w, text);
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
                if report.stop_focus {
                    hub.stop_focus(&w);
                }
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
