//! The Chat screen: conversations with the on-device assistant. The model
//! runs on a worker thread; its reply comes back through `chat-reply`, is
//! stored in the thread, and any proposed changes wait for the person's
//! approval before anything is applied.

use chrono::{Local, NaiveDate};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use thulli_core::ai::pack;
use thulli_core::services::assistant::ChatEngine;

use super::DataHub;
use crate::{AppData, ChatMessageModel, ChatThreadModel, ChatTraceRow, MainWindow};
use thulli_core::research::{self, Depth, EngineBrain, LiveWeb};
use thulli_core::services::preferences::language_name;

fn rows<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// "today", "yesterday" or "date" for a message sent on `day`.
fn day_kind(day: NaiveDate, today: NaiveDate) -> &'static str {
    match (today - day).num_days() {
        0 => "today",
        1 => "yesterday",
        _ => "date",
    }
}

impl DataHub {
    /// Fills the thread list, the open conversation and the waiting proposals.
    pub(super) fn refresh_chat(&self, window: &MainWindow) {
        let data = window.global::<AppData>();
        let current = self.chat_thread.borrow().clone();
        let today = Local::now().date_naive();
        let threads: Vec<ChatThreadModel> = self
            .assistant
            .threads()
            .into_iter()
            .map(|t| {
                // Today shows the time, older chats the date.
                let local = t.updated_at.with_timezone(&Local);
                let label = if local.date_naive() == today { local.format("%H:%M") } else { local.format("%-d %b") };
                ChatThreadModel { id: t.id.into(), title: t.title.into(), updated_label: label.to_string().into() }
            })
            .collect();
        // A day header goes on the first message of each day.
        let mut last_day: Option<NaiveDate> = None;
        let messages: Vec<ChatMessageModel> = current
            .as_deref()
            .map(|id| self.assistant.message_views(id))
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, m)| {
                let local = m.at.with_timezone(&Local);
                let day = local.date_naive();
                let first_of_day = last_day != Some(day);
                last_day = Some(day);
                ChatMessageModel {
                    id: i.to_string().into(),
                    index: i as i32,
                    from_user: m.from_user,
                    text: m.text.into(),
                    time: local.format("%H:%M").to_string().into(),
                    day_kind: if first_of_day { day_kind(day, today) } else { "" }.into(),
                    day_date: if first_of_day { local.format("%a %-d %b").to_string() } else { String::new() }.into(),
                    versions: rows(m.versions.into_iter().map(SharedString::from).collect()),
                }
            })
            .collect();
        data.set_chat_threads(rows(threads));
        data.set_chat_messages(rows(messages));
        data.set_chat_thread(current.unwrap_or_default().into());
        data.set_chat_proposals(rows(self.assistant.pending_descriptions().into_iter().map(SharedString::from).collect()));
    }

    /// Runs the model on the latest message of `thread` in the background.
    fn run_chat(&self, window: &MainWindow, thread: String) {
        let data = window.global::<AppData>();
        data.set_chat_busy(true);
        data.set_chat_status("".into());
        data.set_chat_phase("loading".into());
        self.refresh_chat(window);

        let (run, stop) = self.new_run();
        let latest = self.chat_run.clone();
        let prompt = self.assistant.thread_prompt(&thread, Local::now());
        let models = self.models.clone();
        let weak = window.as_weak();
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering::Relaxed;
            let current = || latest.load(Relaxed) == run;
            let phase = |name: &'static str| {
                if current() {
                    let _ = weak.upgrade_in_event_loop(move |w| w.global::<AppData>().set_chat_phase(name.into()));
                }
            };
            let status = |text: String| {
                let _ = weak.upgrade_in_event_loop(move |w| w.global::<AppData>().set_chat_status(text.into()));
            };
            let result = models
                .llm(&mut |p| status(p.describe()))
                .map_err(|why| format!("The AI could not start: {why}"))
                .and_then(|llm| {
                    // The first pass reads the whole prompt; the first piece of the
                    // answer arrives when it is done.
                    status(String::new());
                    phase("reading");
                    let mut writing = false;
                    llm.generate(&prompt, &mut |_| {
                        if !writing {
                            writing = true;
                            phase("writing");
                        }
                        !stop.load(Relaxed)
                    })
                    .map_err(|e| e.user_message())
                });
            if !current() {
                return; // stopped: the screen has already moved on
            }
            let (ok, text) = match result {
                Ok(t) => (true, t),
                Err(why) => (false, why),
            };
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.global::<AppData>().set_ai_models_ready(pack::is_ready());
                w.global::<AppData>().invoke_chat_reply(thread.into(), ok, text.into());
            });
        });
    }

    /// Starts a numbered run with a fresh stop switch.
    fn new_run(&self) -> (u64, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *self.chat_stop.borrow_mut() = stop.clone();
        let run = self.chat_run.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        (run, stop)
    }

    /// Shows the research steps so far.
    fn show_trace(&self, window: &MainWindow) {
        let rows_now = self.chat_trace.borrow();
        let from = rows_now.len().saturating_sub(40);
        window.global::<AppData>().set_chat_trace(rows(rows_now[from..].to_vec()));
    }

    /// Turns one research event into a line of the step list.
    fn trace_event(&self, window: &MainWindow, kind: &str, text: &str, detail: &str) {
        {
            let mut list = self.chat_trace.borrow_mut();
            // Whatever was in progress is finished once the next thing starts.
            let finish = |list: &mut Vec<ChatTraceRow>| {
                for r in list.iter_mut().filter(|r| r.status == "running") {
                    r.status = "".into();
                }
            };
            let row = |kind: &str, text: &str, detail: &str, status: &str| ChatTraceRow { kind: kind.into(), text: text.into(), detail: detail.into(), status: status.into() };
            match kind {
                "plan" => {
                    finish(&mut list);
                    list.push(row("plan", text, detail, ""));
                }
                "search" | "read" | "merge" | "write" => {
                    finish(&mut list);
                    list.push(row(kind, text, detail, "running"));
                }
                "found" => {
                    if let Some(r) = list.iter_mut().rev().find(|r| r.kind == "search" && r.text == text) {
                        r.detail = detail.into();
                        r.status = "".into();
                    }
                }
                "note" | "skip" => {
                    if let Some(r) = list.iter_mut().rev().find(|r| r.kind == "read") {
                        r.kind = kind.into();
                        r.text = text.into();
                        r.detail = detail.into();
                        r.status = if kind == "skip" { "pending".into() } else { "".into() };
                    }
                }
                _ => {}
            }
        }
        self.show_trace(window);
    }

    pub(super) fn wire_research(self: &std::rc::Rc<Self>, window: &MainWindow) {
        let data = window.global::<AppData>();
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_research_event(move |kind, text, detail| {
                if let Some(w) = weak.upgrade() {
                    hub.trace_event(&w, &kind, &text, &detail);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_research_done(move |thread, ok, text| {
                let Some(w) = weak.upgrade() else { return };
                let data = w.global::<AppData>();
                data.set_chat_busy(false);
                data.set_chat_phase("".into());
                if ok {
                    hub.assistant.push_assistant(&thread, &text);
                } else {
                    data.set_chat_status(text);
                }
                hub.chat_trace.borrow_mut().clear();
                hub.show_trace(&w);
                hub.refresh_chat(&w);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_research(move |question, depth| {
                let Some(w) = weak.upgrade() else { return };
                let data = w.global::<AppData>();
                if data.get_chat_busy() {
                    return;
                }
                if !hub.handle.read(|s| s.preferences.web_search) {
                    data.set_chat_status("Turn on web search in Settings first.".into());
                    return;
                }
                let current = hub.chat_thread.borrow().clone();
                let thread = match hub.assistant.begin_turn(current.as_deref(), &question) {
                    Ok(id) => id,
                    Err(why) => {
                        data.set_chat_status(why.into());
                        return;
                    }
                };
                *hub.chat_thread.borrow_mut() = Some(thread.clone());
                hub.chat_trace.borrow_mut().clear();
                hub.show_trace(&w);
                data.set_chat_busy(true);
                data.set_chat_status("".into());
                hub.refresh_chat(&w);

                let depth = match depth.as_str() {
                    "deep" => Depth::Deep,
                    "standard" => Depth::Standard,
                    _ => Depth::Quick,
                };
                let (code, language) = hub.handle.read(|s| {
                    let id = s.preferences.language.clone();
                    let name = language_name(&id).to_string();
                    (id, name)
                });
                data.set_chat_phase("loading".into());
                let (run, stop) = hub.new_run();
                let latest = hub.chat_run.clone();
                let models = hub.models.clone();
                let weak = w.as_weak();
                let question = question.to_string();
                std::thread::spawn(move || {
                    let current = || latest.load(std::sync::atomic::Ordering::Relaxed) == run;
                    let send = |kind: &str, a: String, b: String| {
                        if !current() {
                            return;
                        }
                        let kind = kind.to_string();
                        let _ = weak.upgrade_in_event_loop(move |w| w.global::<AppData>().invoke_chat_research_event(kind.into(), a.into(), b.into()));
                    };
                    let finish = |ok: bool, text: String| {
                        if !current() {
                            return;
                        }
                        let thread = thread.clone();
                        let _ = weak.upgrade_in_event_loop(move |w| {
                            w.global::<AppData>().set_ai_models_ready(pack::is_ready());
                            w.global::<AppData>().invoke_chat_research_done(thread.into(), ok, text.into());
                        });
                    };
                    let llm = match models.llm(&mut |p| {
                        let text = p.describe();
                        let _ = weak.upgrade_in_event_loop(move |w| w.global::<AppData>().set_chat_status(text.into()));
                    }) {
                        Ok(l) => l,
                        Err(why) => return finish(false, format!("The AI could not start: {why}")),
                    };
                    if current() {
                        let _ = weak.upgrade_in_event_loop(|w| {
                            w.global::<AppData>().set_chat_status("".into());
                            w.global::<AppData>().set_chat_phase("searching".into());
                        });
                    }
                    let brain = EngineBrain(llm as std::sync::Arc<dyn ChatEngine>);
                    let web = LiveWeb { language: code };
                    let report = research::run(&question, depth, &language, &brain, &web, &stop, &mut |e| {
                        use research::Event as E;
                        match e {
                            E::Round(_) => {}
                            E::Planned(q) => send("plan", q.join(" | "), q.len().to_string()),
                            E::Searching(q) => send("search", q, String::new()),
                            E::Found { query, count } => send("found", query, count.to_string()),
                            E::Reading { url, done, total } => send("read", url, format!("{done}/{total}")),
                            E::Note { n, title, .. } => send("note", title, n.to_string()),
                            E::Skipped { url, why } => send("skip", url, why),
                            E::Merging => send("merge", String::new(), String::new()),
                            E::Writing => send("write", String::new(), String::new()),
                        }
                    });
                    if report.answer.trim().is_empty() {
                        return finish(false, "I could not find enough on the web to answer that.".to_string());
                    }
                    let mut text = report.answer.trim().to_string();
                    if !report.sources.is_empty() {
                        text.push_str("\n\n");
                        for s in &report.sources {
                            text.push_str(&format!("[{}] {} - {}\n", s.n, s.title, s.url));
                        }
                    }
                    finish(true, text.trim_end().to_string());
                });
            });
        }
    }

    pub(super) fn wire_chat(self: &std::rc::Rc<Self>, window: &MainWindow) {
        self.wire_research(window);
        let data = window.global::<AppData>();

        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_new(move || {
                *hub.chat_thread.borrow_mut() = Some(hub.assistant.new_thread());
                if let Some(w) = weak.upgrade() {
                    w.global::<AppData>().set_chat_status("".into());
                    hub.refresh_chat(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_open(move |id| {
                hub.assistant.discard_pending();
                *hub.chat_thread.borrow_mut() = Some(id.to_string());
                if let Some(w) = weak.upgrade() {
                    w.global::<AppData>().set_chat_status("".into());
                    hub.refresh_chat(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_send(move |text| {
                let Some(w) = weak.upgrade() else { return };
                let data = w.global::<AppData>();
                if data.get_chat_busy() {
                    return;
                }
                let current = hub.chat_thread.borrow().clone();
                match hub.assistant.begin_turn(current.as_deref(), &text) {
                    Ok(id) => {
                        *hub.chat_thread.borrow_mut() = Some(id.clone());
                        hub.run_chat(&w, id);
                    }
                    Err(why) => data.set_chat_status(why.into()),
                }
            });
        }
        {
            // Edit a sent message: everything after it goes, and it is answered again.
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_edit(move |index, text| {
                let Some(w) = weak.upgrade() else { return };
                let data = w.global::<AppData>();
                if data.get_chat_busy() {
                    return;
                }
                let Some(thread) = hub.chat_thread.borrow().clone() else { return };
                match hub.assistant.edit_turn(&thread, index.max(0) as usize, &text) {
                    Ok(id) => hub.run_chat(&w, id),
                    Err(why) => data.set_chat_status(why.into()),
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_reply(move |thread, ok, text| {
                let Some(w) = weak.upgrade() else { return };
                let data = w.global::<AppData>();
                data.set_chat_busy(false);
                data.set_chat_phase("".into());
                if ok {
                    data.set_chat_status("".into());
                    let outcome = hub.assistant.finish_turn(&thread, &text);
                    if outcome.nothing_changed {
                        data.set_chat_phase("no-changes".into());
                    }
                } else {
                    data.set_chat_status(text);
                }
                hub.refresh_chat(&w);
            });
        }
        {
            let weak = window.as_weak();
            data.on_chat_copy(move |text| {
                let copied = arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string()));
                if let Some(w) = weak.upgrade() {
                    let note = if copied.is_ok() { "Copied." } else { "Could not copy." };
                    w.global::<AppData>().set_chat_status(note.into());
                    // The note fades after a moment, but only if nothing replaced it.
                    let weak = w.as_weak();
                    slint::Timer::single_shot(std::time::Duration::from_secs(2), move || {
                        if let Some(w) = weak.upgrade() {
                            let data = w.global::<AppData>();
                            if data.get_chat_status() == note {
                                data.set_chat_status("".into());
                            }
                        }
                    });
                }
            });
        }
        {
            // Fork the open conversation up to and including message `upto`
            // (-1: all of it) and continue in the copy.
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_fork(move |upto| {
                let Some(thread) = hub.chat_thread.borrow().clone() else { return };
                if let Some(copy) = hub.assistant.fork_thread(&thread, (upto >= 0).then_some(upto as usize)) {
                    *hub.chat_thread.borrow_mut() = Some(copy);
                }
                if let Some(w) = weak.upgrade() {
                    w.global::<AppData>().set_chat_status("".into());
                    hub.refresh_chat(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_fork_thread(move |id| {
                if let Some(copy) = hub.assistant.fork_thread(&id, None) {
                    *hub.chat_thread.borrow_mut() = Some(copy);
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_chat(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_delete(move |id| {
                hub.assistant.delete_thread(&id);
                if hub.chat_thread.borrow().as_deref() == Some(id.as_str()) {
                    *hub.chat_thread.borrow_mut() = None;
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_chat(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_apply(move || {
                let Some(w) = weak.upgrade() else { return };
                let thread = hub.chat_thread.borrow().clone();
                let report = hub.assistant.apply_pending(thread.as_deref());
                w.global::<AppData>().set_chat_status(report.summary().into());
                if report.stop_focus {
                    hub.stop_focus(&w);
                }
                match report.focus_task {
                    Some(id) => hub.launch_focus(&w, &id),
                    None => hub.refresh_all(&w),
                }
                hub.refresh_chat(&w);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_stop(move || {
                // The run may be in a step that cannot be cut short (loading the model,
                // reading a long prompt), so the screen does not wait for it: the run is
                // told to stop and forgotten, and whatever it sends later is dropped.
                hub.chat_stop.borrow().store(true, std::sync::atomic::Ordering::Relaxed);
                hub.chat_run.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(w) = weak.upgrade() else { return };
                let data = w.global::<AppData>();
                data.set_chat_busy(false);
                data.set_chat_status("".into());
                data.set_chat_phase("stopped".into());
                hub.chat_trace.borrow_mut().clear();
                hub.show_trace(&w);
                hub.refresh_chat(&w);
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_chat_discard(move || {
                hub.assistant.discard_pending();
                if let Some(w) = weak.upgrade() {
                    hub.refresh_chat(&w);
                }
            });
        }
    }
}
