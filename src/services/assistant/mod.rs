//! On-device assistant: conversations, memory, suggestions and the tool layer.
//!
//! Everything runs on this machine. The model sits behind [`ChatEngine`];
//! its output is untrusted and only ever becomes [`ProposedAction`]s that the
//! user must approve before [`AssistantService::apply_pending`] changes data.

mod actions;
pub mod engine;
mod prompt;
mod suggestions;

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local, NaiveDate, Utc};

pub use actions::{parse_reply, ParsedReply, ProposedAction};
pub use engine::{
    load_local_engine, local_model_supported, model_status, ChatEngine, EngineError, EngineMessage, EngineRole,
    MockEngine, ModelStatus,
};
pub use suggestions::Suggestion;

/// The planning prompt for `request` against `store`, as the Plan panel builds it.
pub fn plan_messages(store: &crate::models::AppStateStore, request: &str) -> Vec<EngineMessage> {
    prompt::plan_prompt(store, request, Local::now())
}

/// Like [`plan_messages`] for a given moment; used to build training data.
pub fn plan_messages_at(store: &crate::models::AppStateStore, request: &str, now: DateTime<Local>) -> Vec<EngineMessage> {
    prompt::plan_prompt(store, request, now)
}

/// The chat prompt for one question against `store`: the system prompt with a
/// compact snapshot of the user's data, then the question.
pub fn chat_messages(store: &crate::models::AppStateStore, request: &str) -> Vec<EngineMessage> {
    vec![
        EngineMessage::new(EngineRole::System, prompt::system_prompt(store, Local::now())),
        EngineMessage::new(EngineRole::User, request.to_string()),
    ]
}

use crate::models::{
    AssistantMemory, ChatRole, ChatThread, Goal, MemoryKind, Milestone, Note, SuggestionKind, SuggestionState, Task, TaskList,
};
use crate::services::store_handle::StoreHandle;

/// Longest message the user can send.
pub const MAX_USER_CHARS: usize = 4000;
/// Longest memory text.
pub const MAX_MEMORY_CHARS: usize = 500;
const TITLE_CHARS: usize = 40;

/// A thread in the history list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
}

/// One message as the Chat screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageView {
    pub from_user: bool,
    pub text: String,
    pub at: DateTime<Utc>,
    /// Earlier replies to the same turn, oldest first.
    pub versions: Vec<String>,
}

/// What came back from one assistant turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    /// The reply as the user reads it (action block removed).
    pub text: String,
    /// Plain-language lines for each proposed change; empty when none.
    pub proposals: Vec<String>,
    /// The reply tried to change something but nothing usable came of it, so the
    /// screen says that no changes were made.
    pub nothing_changed: bool,
}

/// What happened when proposals were applied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyReport {
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
    /// The task a `start_focus` action asked for, once it exists.
    pub focus_task: Option<String>,
    /// A `stop_focus` action asked to stop the timer.
    pub stop_focus: bool,
}

impl ApplyReport {
    /// One short sentence for the thread and the status line.
    pub fn summary(&self) -> String {
        let mut s = match self.applied.len() {
            0 => "Nothing was changed.".to_string(),
            1 => "Done: 1 change made.".to_string(),
            n => format!("Done: {n} changes made."),
        };
        if !self.skipped.is_empty() {
            s.push_str(&format!(" Skipped {}: {}.", self.skipped.len(), self.skipped.join("; ")));
        }
        s
    }
}

/// On-device assistant conversations.
pub struct AssistantService {
    handle: StoreHandle,
    pending: Mutex<Vec<ProposedAction>>,
    /// Earlier replies carried onto the next reply after an edit.
    carry: Mutex<Vec<String>>,
    engine: Mutex<EngineSlot>,
}

/// The loaded engine, remembered with the path it came from so a changed
/// preference reloads it.
#[derive(Default)]
struct EngineSlot {
    forced: Option<Arc<dyn ChatEngine>>,
    loaded: Option<(String, Arc<dyn ChatEngine>)>,
}

impl AssistantService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle, pending: Mutex::new(Vec::new()), carry: Mutex::new(Vec::new()), engine: Mutex::new(EngineSlot::default()) }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    /// Uses `engine` instead of loading the model file: a hosted model, or
    /// a scripted one in tests.
    pub fn set_engine(&self, engine: Arc<dyn ChatEngine>) {
        self.engine.lock().unwrap().forced = Some(engine);
    }

    /// Goes back to the model file from preferences, if any.
    pub fn clear_engine(&self) {
        self.engine.lock().unwrap().forced = None;
    }

    // ---- model --------------------------------------------------------

    /// State of the model chosen in preferences.
    pub fn model_status(&self) -> ModelStatus {
        if self.engine.lock().unwrap().forced.is_some() {
            return ModelStatus::Ready;
        }
        let path = self.handle.read(|s| s.preferences.assistant_model_path.clone());
        model_status(&path)
    }

    /// The engine to use, loading the model file if needed. Blocks while the
    /// model loads, so call it from a worker thread.
    fn engine(&self) -> Result<Arc<dyn ChatEngine>, EngineError> {
        let path = self.handle.read(|s| s.preferences.assistant_model_path.trim().to_string());
        {
            // The lock is released before loading so `model_status` on the UI
            // thread never waits for a slow model load.
            let slot = self.engine.lock().unwrap();
            if let Some(e) = &slot.forced {
                return Ok(e.clone());
            }
            if let Some((loaded_path, e)) = &slot.loaded {
                if *loaded_path == path {
                    return Ok(e.clone());
                }
            }
        }
        let engine = load_local_engine(&path)?;
        self.engine.lock().unwrap().loaded = Some((path, engine.clone()));
        Ok(engine)
    }

    // ---- threads ------------------------------------------------------

    /// Threads, most recently active first.
    pub fn threads(&self) -> Vec<ThreadSummary> {
        self.handle.read(|s| {
            let mut v: Vec<ThreadSummary> = s
                .chat_threads
                .iter()
                .map(|t| ThreadSummary { id: t.id.clone(), title: display_title(t), updated_at: t.updated_at })
                .collect();
            v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            v
        })
    }

    /// Messages of a thread as (is_from_user, text), oldest first.
    pub fn messages(&self, thread_id: &str) -> Vec<(bool, String)> {
        self.handle.read(|s| {
            s.chat_threads
                .iter()
                .find(|t| t.id == thread_id)
                .map(|t| t.messages.iter().map(|m| (m.role == ChatRole::User, m.text.clone())).collect())
                .unwrap_or_default()
        })
    }

    /// Starts an empty thread and returns its id.
    pub fn new_thread(&self) -> String {
        self.discard_pending();
        self.handle.mutate(|s| {
            let t = ChatThread::new("");
            let id = t.id.clone();
            s.chat_threads.push(t);
            id
        })
    }

    /// Records the user's message, creating a thread when `thread_id` is
    /// `None` or unknown. Returns the thread id, or why the text was refused.
    pub fn begin_turn(&self, thread_id: Option<&str>, text: &str) -> Result<String, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Type a message first.".into());
        }
        if text.chars().count() > MAX_USER_CHARS {
            return Err("That message is too long.".into());
        }
        self.discard_pending();
        self.carry.lock().unwrap().clear();
        Ok(self.handle.mutate(|s| {
            let idx = thread_id.and_then(|id| s.chat_threads.iter().position(|t| t.id == id)).unwrap_or_else(|| {
                s.chat_threads.push(ChatThread::new(""));
                s.chat_threads.len() - 1
            });
            let t = &mut s.chat_threads[idx];
            if t.title.is_empty() {
                t.title = title_from(text);
            }
            t.push(ChatRole::User, text);
            t.id.clone()
        }))
    }

    /// Asks the model to answer the latest message in a thread. Blocking:
    /// run it on a worker thread. `on_token` gets text as it streams and may
    /// return `false` to stop.
    ///
    /// The reply is stored in the thread with any action block removed; valid
    /// proposals are held until [`apply_pending`](Self::apply_pending) or
    /// [`discard_pending`](Self::discard_pending).
    pub fn respond(
        &self,
        thread_id: &str,
        now: DateTime<Local>,
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<TurnOutcome, EngineError> {
        let engine = self.engine()?;
        let prompt = self.thread_prompt(thread_id, now);
        let raw = engine.generate(&prompt, on_token)?;
        Ok(self.finish_turn(thread_id, &raw))
    }

    /// The prompt for answering the latest message of a thread.
    pub fn thread_prompt(&self, thread_id: &str, now: DateTime<Local>) -> Vec<EngineMessage> {
        self.handle.read(|s| prompt::build_prompt(s, thread_id, now))
    }

    /// Stores the model's raw reply in the thread and holds its valid
    /// proposals for the user's decision. Split from [`respond`](Self::respond)
    /// so a caller can run the model on another thread.
    pub fn finish_turn(&self, thread_id: &str, raw: &str) -> TurnOutcome {
        let mut parsed = parse_reply(crate::ai::llm::strip_thinking(raw));
        if parsed.text.is_empty() {
            parsed.text = if parsed.actions.is_empty() {
                "I do not have an answer for that.".to_string()
            } else {
                "Here is what I suggest.".to_string()
            };
        }
        let proposals: Vec<String> = parsed.actions.iter().map(ProposedAction::describe).collect();
        self.handle.mutate(|s| {
            if let Some(t) = s.chat_threads.iter_mut().find(|t| t.id == thread_id) {
                t.push(ChatRole::Assistant, parsed.text.clone());
                if let Some(m) = t.messages.last_mut() {
                    m.versions = std::mem::take(&mut *self.carry.lock().unwrap());
                }
            }
        });
        *self.pending.lock().unwrap() = parsed.actions;
        TurnOutcome { nothing_changed: proposals.is_empty() && (parsed.attempted || !parsed.rejected.is_empty()), text: parsed.text, proposals }
    }

    /// Messages of a thread with their times and older replies.
    pub fn message_views(&self, thread_id: &str) -> Vec<MessageView> {
        self.handle.read(|s| {
            s.chat_threads
                .iter()
                .find(|t| t.id == thread_id)
                .map(|t| {
                    t.messages
                        .iter()
                        .map(|m| MessageView { from_user: m.role == ChatRole::User, text: m.text.clone(), at: m.created_at, versions: m.versions.clone() })
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// Adds a reply written outside the model call (a finished research).
    pub fn push_assistant(&self, thread_id: &str, text: &str) {
        self.handle.mutate(|s| {
            if let Some(t) = s.chat_threads.iter_mut().find(|t| t.id == thread_id) {
                t.push(ChatRole::Assistant, text);
            }
        });
    }

    /// Replaces the text of the sent message at `index` and drops everything
    /// after it. The reply it had (and that reply's older versions) is kept
    /// and attached to the new reply, so the person can still read it.
    /// Returns the thread id; run the model next, as after `begin_turn`.
    pub fn edit_turn(&self, thread_id: &str, index: usize, text: &str) -> Result<String, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Type a message first.".into());
        }
        if text.chars().count() > MAX_USER_CHARS {
            return Err("That message is too long.".into());
        }
        self.discard_pending();
        let carried = self.handle.mutate(|s| {
            let t = s.chat_threads.iter_mut().find(|t| t.id == thread_id).ok_or_else(|| "That conversation is gone.".to_string())?;
            if t.messages.get(index).map(|m| m.role) != Some(ChatRole::User) {
                return Err("Only a sent message can be edited.".to_string());
            }
            let mut carried = Vec::new();
            if let Some(old) = t.messages.get(index + 1).filter(|m| m.role == ChatRole::Assistant) {
                carried = old.versions.clone();
                carried.push(old.text.clone());
            }
            t.messages.truncate(index + 1);
            t.messages[index].text = text.to_string();
            t.updated_at = Utc::now();
            Ok(carried)
        })?;
        *self.carry.lock().unwrap() = carried;
        Ok(thread_id.to_string())
    }

    /// Removes a conversation.
    pub fn delete_thread(&self, thread_id: &str) -> bool {
        self.discard_pending();
        self.handle.mutate(|s| {
            let before = s.chat_threads.len();
            s.chat_threads.retain(|t| t.id != thread_id);
            s.chat_threads.len() != before
        })
    }

    /// Copies a conversation, up to and including message `upto` (all of it
    /// when `None`), into a new one and returns the new id.
    pub fn fork_thread(&self, thread_id: &str, upto: Option<usize>) -> Option<String> {
        self.discard_pending();
        self.handle.mutate(|s| {
            let src = s.chat_threads.iter().find(|t| t.id == thread_id)?.clone();
            let keep = upto.map_or(src.messages.len(), |i| (i + 1).min(src.messages.len()));
            let mut copy = ChatThread::new(src.title.clone());
            copy.messages = src.messages.into_iter().take(keep).map(|mut m| {
                m.id = uuid::Uuid::new_v4().to_string();
                m
            }).collect();
            let id = copy.id.clone();
            s.chat_threads.push(copy);
            Some(id)
        })
    }

    // ---- proposals ----------------------------------------------------

    /// Plain-language lines for the changes waiting for the user's decision.
    pub fn pending_descriptions(&self) -> Vec<String> {
        self.pending.lock().unwrap().iter().map(ProposedAction::describe).collect()
    }

    pub fn discard_pending(&self) {
        self.pending.lock().unwrap().clear();
    }

    /// Applies the waiting proposals after the user agreed, then clears them.
    /// Proposals that point at something that does not exist are skipped.
    pub fn apply_pending(&self, thread_id: Option<&str>) -> ApplyReport {
        let actions = std::mem::take(&mut *self.pending.lock().unwrap());
        let report = self.apply_actions(&actions);
        if let Some(id) = thread_id {
            let summary = report.summary();
            self.handle.mutate(|s| {
                if let Some(t) = s.chat_threads.iter_mut().find(|t| t.id == id) {
                    t.push(ChatRole::Assistant, summary);
                }
            });
        }
        report
    }

    /// Applies `actions` in order in one saved change.
    pub fn apply_actions(&self, actions: &[ProposedAction]) -> ApplyReport {
        let mut report = ApplyReport::default();
        if actions.is_empty() {
            return report;
        }
        self.handle.mutate(|store| {
            for action in actions {
                if let ProposedAction::StartFocus { task } = action {
                    // Started by the shell once the changes are saved.
                    match resolve_task(store, task) {
                        Ok(id) => {
                            report.focus_task = Some(id);
                            report.applied.push(action.describe());
                        }
                        Err(why) => report.skipped.push(why),
                    }
                    continue;
                }
                if let ProposedAction::StopFocus = action {
                    // Also done by the shell, which owns the timer.
                    report.stop_focus = true;
                    report.applied.push(action.describe());
                    continue;
                }
                match apply_one(store, action) {
                    Ok(()) => report.applied.push(action.describe()),
                    Err(why) => report.skipped.push(why),
                }
            }
        });
        report
    }

    // ---- one-shot planning ---------------------------------------------

    /// The engine to send a request to. Blocks while a local model loads,
    /// so call it from a worker thread.
    pub fn engine_for_request(&self) -> Result<Arc<dyn ChatEngine>, EngineError> {
        self.engine()
    }

    /// The prompt that turns a plain request ("plan my afternoon: ...") into
    /// actions. One call, JSON only, so small local models handle it too.
    pub fn plan_prompt(&self, request: &str, now: DateTime<Local>) -> Result<Vec<EngineMessage>, String> {
        let request = request.trim();
        if request.is_empty() {
            return Err("Type what you want to get done first.".into());
        }
        if request.chars().count() > MAX_USER_CHARS {
            return Err("That request is too long.".into());
        }
        Ok(self.handle.read(|s| prompt::plan_prompt(s, request, now)))
    }

    /// The prompt that sorts every unsorted open task into a quadrant.
    pub fn sort_prompt(&self, now: DateTime<Local>) -> Vec<EngineMessage> {
        self.handle.read(|s| prompt::sort_prompt(s, now))
    }

    /// Reads a reply to a plan or sort prompt and holds its valid actions
    /// for the user's decision. Returns the plain-language lines.
    pub fn accept_reply(&self, raw: &str) -> Vec<String> {
        self.accept_reply_with_reasons(raw).0
    }

    /// Like [`accept_reply`], also returning why any proposal was refused.
    pub fn accept_reply_with_reasons(&self, raw: &str) -> (Vec<String>, Vec<String>) {
        let parsed = parse_reply(crate::ai::llm::strip_thinking(raw));
        let lines = parsed.actions.iter().map(ProposedAction::describe).collect();
        *self.pending.lock().unwrap() = parsed.actions;
        (lines, parsed.rejected)
    }

    /// The quadrants the waiting actions propose for existing tasks, as
    /// (task id, quadrant). References that match no task are left out.
    pub fn pending_quadrants(&self) -> Vec<(String, crate::models::MatrixQuadrant)> {
        let pending = self.pending.lock().unwrap().clone();
        self.handle.read(|s| {
            pending
                .iter()
                .filter_map(|a| match a {
                    ProposedAction::SetQuadrant { task, quadrant } => resolve_task(s, task).ok().map(|id| (id, *quadrant)),
                    _ => None,
                })
                .collect()
        })
    }

    // ---- summary and quick actions -----------------------------------

    /// A friendly summary of today, computed without the model.
    pub fn day_summary(&self, today: NaiveDate) -> String {
        self.handle.read(|s| prompt::day_summary(s, today))
    }

    /// The opening message for a quick action, or `None` for an unknown id.
    pub fn quick_action_prompt(action: &str) -> Option<&'static str> {
        match action {
            "quick_prioritize" => Some("Quick Prioritize: look at my open tasks and tell me what to do first, and why."),
            "wind_down_reflection" => {
                Some("Wind-down Reflection: help me look back on today, what went well and what to carry over.")
            }
            _ => None,
        }
    }

    // ---- memory -------------------------------------------------------

    /// Memories the user has not removed, newest edit first.
    pub fn memories(&self) -> Vec<AssistantMemory> {
        self.handle.read(|s| {
            let mut v: Vec<AssistantMemory> = s.assistant_memories.iter().filter(|m| !m.archived).cloned().collect();
            v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            v
        })
    }

    pub fn add_memory(&self, kind: &str, text: &str) -> Result<String, String> {
        let text = clean_memory(text)?;
        let kind = parse_memory_kind(kind).ok_or_else(|| "Pick what kind of memory this is.".to_string())?;
        Ok(self.handle.mutate(|s| {
            let m = AssistantMemory::new(kind, text);
            let id = m.id.clone();
            s.assistant_memories.push(m);
            id
        }))
    }

    /// Changes a memory's wording; the old wording is kept in its history.
    pub fn edit_memory(&self, id: &str, text: &str) -> Result<(), String> {
        let text = clean_memory(text)?;
        self.handle.mutate(|s| match s.assistant_memories.iter_mut().find(|m| m.id == id && !m.archived) {
            Some(m) if m.text != text => {
                m.edit(text);
                Ok(())
            }
            Some(_) => Ok(()),
            None => Err("That memory no longer exists.".to_string()),
        })
    }

    /// Removes a memory from use (kept archived so its history is not lost).
    pub fn remove_memory(&self, id: &str) -> bool {
        self.handle.mutate(|s| match s.assistant_memories.iter_mut().find(|m| m.id == id && !m.archived) {
            Some(m) => {
                m.archive();
                true
            }
            None => false,
        })
    }

    /// Earlier wordings of a memory, oldest first.
    pub fn memory_history(&self, id: &str) -> Vec<String> {
        self.handle.read(|s| {
            s.assistant_memories.iter().find(|m| m.id == id).map(|m| m.history.iter().map(|h| h.text.clone()).collect()).unwrap_or_default()
        })
    }

    // ---- suggestions --------------------------------------------------

    /// Suggestions to show now. Always empty while `focus_active`.
    pub fn suggestions(&self, now: DateTime<Local>, focus_active: bool) -> Vec<Suggestion> {
        self.handle.read(|s| suggestions::compute(s, now, focus_active))
    }

    pub fn suggestion_id(kind: SuggestionKind) -> &'static str {
        suggestions::kind_id(kind)
    }

    /// Hides a suggestion for a while.
    pub fn dismiss_suggestion(&self, id: &str, now: DateTime<Local>) {
        self.snooze(id, now, suggestions::dismiss_duration());
    }

    /// Accepts a suggestion: hides it for longer and returns the message to
    /// send to the assistant, if it has one.
    pub fn accept_suggestion(&self, id: &str, now: DateTime<Local>) -> Option<&'static str> {
        let kind = suggestions::parse_kind(id)?;
        self.snooze(id, now, suggestions::accept_duration());
        suggestions::accept_prompt(kind)
    }

    fn snooze(&self, id: &str, now: DateTime<Local>, by: chrono::Duration) {
        let Some(kind) = suggestions::parse_kind(id) else { return };
        let until = now.with_timezone(&Utc) + by;
        self.handle.mutate(|s| {
            if let Some(st) = s.suggestion_states.iter_mut().find(|st| st.kind == kind) {
                st.dismiss_until(until);
            } else {
                let mut st = SuggestionState::new(kind);
                st.dismiss_until(until);
                s.suggestion_states.push(st);
            }
        });
    }
}

fn parse_memory_kind(id: &str) -> Option<MemoryKind> {
    [MemoryKind::Preference, MemoryKind::Commitment, MemoryKind::PersonOrProject, MemoryKind::Pattern]
        .into_iter()
        .find(|k| k.as_str() == id)
}

fn clean_memory(text: &str) -> Result<String, String> {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return Err("Write something to remember.".into());
    }
    if t.chars().count() > MAX_MEMORY_CHARS {
        return Err("That memory is too long.".into());
    }
    Ok(t)
}

fn title_from(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= TITLE_CHARS {
        flat
    } else {
        let cut: String = flat.chars().take(TITLE_CHARS).collect();
        format!("{}...", cut.trim_end())
    }
}

fn display_title(t: &ChatThread) -> String {
    if t.title.is_empty() { "New chat".to_string() } else { t.title.clone() }
}

/// Finds a task by id, by the short id shown to the model (the start of its
/// id), or by exact title (ignoring case) among tasks still to do.
fn resolve_task(store: &crate::models::AppStateStore, reference: &str) -> Result<String, String> {
    if let Some(t) = store.task(reference) {
        return Ok(t.id.clone());
    }
    let short = reference.trim().trim_start_matches('#');
    if short.len() >= prompt::SHORT_ID_LEN && short.chars().all(|c| c.is_ascii_hexdigit()) {
        let hits: Vec<&Task> = store.tasks.iter().filter(|t| t.id.starts_with(short)).collect();
        if let [one] = hits.as_slice() {
            return Ok(one.id.clone());
        }
    }
    let wanted = reference.trim().to_lowercase();
    let named = |t: &&Task| t.title.trim().to_lowercase() == wanted;
    let mut matches: Vec<&Task> = store.tasks.iter().filter(|t| t.is_pending()).filter(named).collect();
    if matches.is_empty() {
        matches = store.tasks.iter().filter(named).collect();
    }
    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!("no task called \"{reference}\"")),
        _ => Err(format!("more than one task called \"{reference}\"")),
    }
}

/// A space by name (any case), or by id.
fn resolve_space(store: &crate::models::AppStateStore, reference: &str) -> Result<String, String> {
    let wanted = reference.trim().to_lowercase();
    store
        .lists
        .iter()
        .find(|l| !l.is_archived && (l.id == reference || l.name.trim().to_lowercase() == wanted))
        .map(|l| l.id.clone())
        .ok_or_else(|| format!("no space called \"{reference}\""))
}

/// The space to put a new task in: the named one, created if missing, or the
/// first space, or a new "Tasks" space when there are none yet.
fn space_for_new_task(store: &mut crate::models::AppStateStore, name: Option<&String>) -> String {
    if let Some(name) = name {
        if let Ok(id) = resolve_space(store, name) {
            return id;
        }
        return store.add_list(TaskList::new(name.trim(), SPACE_COLOR));
    }
    if let Some(l) = store.get_primary_list() {
        return l.id.clone();
    }
    store.add_list(TaskList::new("Tasks", SPACE_COLOR))
}

/// Colour for spaces the assistant creates without one.
const SPACE_COLOR: &str = "#8B5CF6";

fn apply_one(store: &mut crate::models::AppStateStore, action: &ProposedAction) -> Result<(), String> {
    match action {
        ProposedAction::CreateTask { title, section, estimate_mins, list, quadrant, do_date, deadline, steps } => {
            let list_id = space_for_new_task(store, list.as_ref());
            let mut task = Task::new(list_id, title.clone(), *estimate_mins, *section);
            task.sort_order = store.tasks.iter().map(|t| t.sort_order).max().map_or(0, |m| m + 1);
            task.matrix_quadrant = *quadrant;
            task.do_date = *do_date;
            task.deadline = *deadline;
            let task = store.add_task(task);
            for (i, step_title) in steps.iter().enumerate() {
                let mut step = Task::new_step(&task, step_title.clone());
                step.sort_order = i as i32;
                store.add_task(step);
            }
            Ok(())
        }
        ProposedAction::AddSubtask { task, title } => {
            let parent_id = resolve_task(store, task)?;
            let parent = store.task(&parent_id).cloned().ok_or_else(|| format!("no task called \"{task}\""))?;
            let mut step = Task::new_step(&parent, title.clone());
            step.sort_order = store.steps_of(&parent_id).len() as i32;
            store.add_task(step);
            Ok(())
        }
        ProposedAction::CreateNote { title, body } => {
            let mut note = Note::new(title.clone());
            note.set_body(body.clone());
            store.notes.push(note);
            Ok(())
        }
        ProposedAction::ScheduleTask { task, at } => {
            let id = resolve_task(store, task)?;
            let t = store.task_mut(&id).ok_or_else(|| format!("no task called \"{task}\""))?;
            t.scheduled_at = Some(*at);
            t.record("Scheduled by the assistant");
            Ok(())
        }
        ProposedAction::SetQuadrant { task, quadrant } => {
            let id = resolve_task(store, task)?;
            let t = store.task_mut(&id).ok_or_else(|| format!("no task called \"{task}\""))?;
            t.matrix_quadrant = Some(*quadrant);
            t.move_to_section(quadrant.section());
            Ok(())
        }
        ProposedAction::UpdateTask { task, title, estimate_mins, section, notes, do_date, deadline, list } => {
            let id = resolve_task(store, task)?;
            let list_id = match list {
                Some(name) => Some(resolve_space(store, name)?),
                None => None,
            };
            let t = store.task_mut(&id).ok_or_else(|| format!("no task called \"{task}\""))?;
            if let Some(v) = title {
                t.title = v.clone();
            }
            if let Some(v) = estimate_mins {
                t.estimate_mins = *v;
            }
            if let Some(v) = section {
                t.move_to_section(*v);
            }
            if let Some(v) = notes {
                t.notes = v.clone();
            }
            if let Some(v) = do_date {
                t.do_date = *v;
            }
            if let Some(v) = deadline {
                t.deadline = *v;
            }
            if let Some(v) = list_id {
                t.list_id = v;
            }
            t.record("Edited by the assistant");
            Ok(())
        }
        ProposedAction::CompleteTask { task, done } => {
            let id = resolve_task(store, task)?;
            let t = store.task_mut(&id).ok_or_else(|| format!("no task called \"{task}\""))?;
            if *done {
                t.complete();
            } else {
                t.reopen();
            }
            Ok(())
        }
        ProposedAction::DeleteTask { task } => {
            let id = resolve_task(store, task)?;
            if store.remove_task(&id) { Ok(()) } else { Err(format!("no task called \"{task}\"")) }
        }
        ProposedAction::CreateSpace { name, color } => {
            if resolve_space(store, name).is_ok() {
                return Err(format!("a space called \"{name}\" already exists"));
            }
            store.add_list(TaskList::new(name.clone(), color.as_deref().unwrap_or(SPACE_COLOR)));
            Ok(())
        }
        ProposedAction::RenameSpace { space, name } => {
            let id = resolve_space(store, space)?;
            if let Some(l) = store.lists.iter_mut().find(|l| l.id == id) {
                l.name = name.clone();
            }
            Ok(())
        }
        ProposedAction::DeleteSpace { space } => {
            let id = resolve_space(store, space)?;
            store.remove_list_cascade(&id);
            Ok(())
        }
        ProposedAction::CreateGoal { title, description, motivation, category, priority, target_date, milestones, tasks } => {
            let same = |g: &Goal| g.title.trim().eq_ignore_ascii_case(title.trim());
            if store.goals.iter().any(|g| same(g) && g.status != crate::models::GoalStatus::Archived) {
                return Err(format!("a goal called \"{title}\" already exists"));
            }
            let mut goal = Goal::new(title.clone());
            goal.description = description.clone();
            goal.motivation = motivation.clone();
            goal.category = category.clone();
            goal.priority = *priority;
            goal.target_date = *target_date;
            for m in milestones {
                let mut ms = Milestone::new(m.title.clone());
                ms.target_date = m.date;
                goal.milestones.push(ms);
            }
            goal.renumber_milestones();
            if !tasks.is_empty() {
                let list_id = space_for_new_task(store, None);
                let mut order = store.tasks.iter().map(|t| t.sort_order).max().map_or(0, |m| m + 1);
                for t in tasks {
                    let mut task = Task::new(list_id.clone(), t.title.clone(), t.estimate_mins, crate::models::TaskSection::ThisWeek);
                    task.sort_order = order;
                    order += 1;
                    task.goal_id = Some(goal.id.clone());
                    task.milestone_id = t.milestone.as_deref().and_then(|name| {
                        goal.milestones.iter().find(|m| m.title.trim().eq_ignore_ascii_case(name.trim())).map(|m| m.id.clone())
                    });
                    store.add_task(task);
                }
            }
            store.goals.push(goal);
            Ok(())
        }
        ProposedAction::LinkTask { goal, task, milestone } => {
            let wanted = goal.trim().to_lowercase();
            let found = store
                .goals
                .iter()
                .find(|g| g.status != crate::models::GoalStatus::Archived && g.title.trim().to_lowercase() == wanted)
                .ok_or_else(|| format!("no goal called \"{goal}\""))?;
            let goal_id = found.id.clone();
            let milestone_id = match milestone {
                Some(name) => Some(
                    found
                        .milestones
                        .iter()
                        .find(|m| m.title.trim().eq_ignore_ascii_case(name.trim()))
                        .map(|m| m.id.clone())
                        .ok_or_else(|| format!("no milestone called \"{name}\""))?,
                ),
                None => None,
            };
            let task_id = resolve_task(store, task)?;
            // A step links through its top-level task.
            let mut root = task_id;
            while let Some(parent) = store.task(&root).and_then(|t| t.parent_id.clone()) {
                root = parent;
            }
            let t = store.task_mut(&root).ok_or_else(|| format!("no task called \"{task}\""))?;
            t.goal_id = Some(goal_id);
            t.milestone_id = milestone_id;
            t.record("Linked to a goal by the assistant");
            Ok(())
        }
        ProposedAction::LogTime { task, minutes, date } => {
            let id = resolve_task(store, task)?;
            let secs = u64::from(*minutes) * 60;
            let end = match date {
                Some(d) => d
                    .and_hms_opt(12, 0, 0)
                    .and_then(|dt| dt.and_local_timezone(Local).earliest())
                    .map(|t| t.with_timezone(&Utc))
                    .unwrap_or_else(Utc::now),
                None => Utc::now(),
            };
            let start = end - chrono::Duration::seconds(secs as i64);
            let mut entry = crate::models::TimeEntry::new(crate::models::TimeEntryKind::Focus, start, end);
            entry.task_id = Some(id.clone());
            store.time_entries.push(entry);
            let t = store.task_mut(&id).ok_or_else(|| format!("no task called \"{task}\""))?;
            t.add_time(secs);
            t.record("Time logged by the assistant");
            Ok(())
        }
        // Handled in apply_actions; they change no data.
        ProposedAction::StartFocus { .. } | ProposedAction::StopFocus => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AppStateStore, TaskSection};
    use crate::repository::StorageRepository;
    use chrono::TimeZone;

    struct NullRepo;
    impl StorageRepository for NullRepo {
        fn load_with_report(&self) -> crate::repository::LoadReport {
            crate::repository::LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    fn service() -> AssistantService {
        let store = Arc::new(Mutex::new(AppStateStore::with_space()));
        AssistantService::new(StoreHandle::new(store, Arc::new(NullRepo)))
    }

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 5, 4, 9, 0, 0).unwrap()
    }

    #[test]
    fn assistant_manages_tasks_subtasks_spaces_and_the_timer() {
        let svc = service();
        let (task, step) = svc.handle().mutate(|s| {
            let list = s.lists[0].id.clone();
            let t = s.add_task(Task::new(list, "Read notes", 0, TaskSection::Backlog));
            let st = s.add_task(Task::new_step(&t, "Chapter 1"));
            (t.id, st.id)
        });
        let reply = format!(
            "{{\"actions\":[\
            {{\"type\":\"create_task\",\"title\":\"Revise\",\"space\":\"Exams\",\"steps\":[\"A\"]}},\
            {{\"type\":\"update_task\",\"task\":\"{t}\",\"title\":\"Read all notes\",\"estimate_mins\":50}},\
            {{\"type\":\"complete_task\",\"task\":\"{st}\"}},\
            {{\"type\":\"rename_space\",\"space\":\"Exams\",\"name\":\"Finals\"}},\
            {{\"type\":\"stop_focus\"}}]}}",
            t = &task[..prompt::SHORT_ID_LEN],
            st = &step[..prompt::SHORT_ID_LEN],
        );
        assert_eq!(svc.accept_reply(&reply).len(), 5);
        let report = svc.apply_pending(None);
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert!(report.stop_focus);
        svc.handle().read(|s| {
            let t = s.task(&task).unwrap();
            assert_eq!((t.title.as_str(), t.estimate_mins), ("Read all notes", 50));
            assert!(s.task(&step).unwrap().is_completed());
            let finals = s.lists.iter().find(|l| l.name == "Finals").expect("space created then renamed");
            assert!(s.tasks.iter().any(|t| t.title == "Revise" && t.list_id == finals.id));
        });

        // Deleting a subtask and a space.
        let reply = format!(
            "{{\"actions\":[{{\"type\":\"delete_task\",\"task\":\"{}\"}},{{\"type\":\"delete_space\",\"space\":\"finals\"}}]}}",
            &step[..prompt::SHORT_ID_LEN]
        );
        svc.accept_reply(&reply);
        assert!(svc.apply_pending(None).skipped.is_empty());
        svc.handle().read(|s| {
            assert!(s.task(&step).is_none());
            assert!(!s.lists.iter().any(|l| l.name == "Finals"));
            assert!(!s.tasks.iter().any(|t| t.title == "Revise"));
        });
    }

    #[test]
    fn goal_plans_create_a_goal_with_milestones_and_linked_tasks() {
        let svc = service();
        let reply = r#"{"actions":[{"type":"create_goal","title":"Finish thesis","description":"Write it","priority":"high",
            "target_date":"2027-02-01","category":"Study",
            "milestones":["Outline",{"title":"Draft","date":"2026-12-01"},{"title":"Final","date":"not a date"}],
            "tasks":[{"title":"Write intro","estimate_mins":90,"milestone":"outline"},"Collect sources",{"title":"Polish","milestone":"Missing"}]}]}"#;
        assert_eq!(svc.accept_reply(reply).len(), 1);
        let report = svc.apply_pending(None);
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        svc.handle().read(|s| {
            assert_eq!(s.goals.len(), 1);
            let g = &s.goals[0];
            assert_eq!((g.title.as_str(), g.priority), ("Finish thesis", crate::models::GoalPriority::High));
            assert_eq!(g.milestones.len(), 3);
            assert!(g.milestones[2].target_date.is_none(), "a bad milestone date is dropped, not the milestone");
            let linked = crate::services::goals::linked_tasks(s, &g.id);
            assert_eq!(linked.len(), 3);
            let intro = linked.iter().find(|t| t.title == "Write intro").unwrap();
            assert_eq!(intro.milestone_id.as_deref(), Some(g.milestones[0].id.as_str()));
            assert!(linked.iter().find(|t| t.title == "Polish").unwrap().milestone_id.is_none());
            assert_eq!(intro.estimate_mins, 90);
        });
    }

    #[test]
    fn a_second_goal_with_the_same_title_is_refused_and_link_task_works() {
        let svc = service();
        svc.apply_actions(&[ProposedAction::CreateGoal {
            title: "Read".into(),
            description: String::new(),
            motivation: String::new(),
            category: String::new(),
            priority: crate::models::GoalPriority::Medium,
            target_date: None,
            milestones: vec![actions::MilestoneDraft { title: "Half".into(), date: None }],
            tasks: vec![],
        }]);
        let r = svc.apply_actions(&[ProposedAction::CreateGoal {
            title: " read ".into(),
            description: String::new(),
            motivation: String::new(),
            category: String::new(),
            priority: crate::models::GoalPriority::Medium,
            target_date: None,
            milestones: vec![],
            tasks: vec![],
        }]);
        assert_eq!(r.applied.len(), 0);
        assert_eq!(r.skipped.len(), 1);
        let task_title = svc.handle().mutate(|s| {
            let list = s.lists[0].id.clone();
            s.add_task(Task::new(list, "Chapter one", 30, TaskSection::Today)).title
        });
        let r = svc.apply_actions(&[ProposedAction::LinkTask { goal: "READ".into(), task: task_title.clone(), milestone: Some("half".into()) }]);
        assert!(r.skipped.is_empty(), "{:?}", r.skipped);
        svc.handle().read(|s| {
            let t = s.tasks.iter().find(|t| t.title == task_title).unwrap();
            assert_eq!(t.goal_id.as_deref(), Some(s.goals[0].id.as_str()));
            assert!(t.milestone_id.is_some());
        });
        let r = svc.apply_actions(&[ProposedAction::LinkTask { goal: "Nope".into(), task: task_title, milestone: None }]);
        assert_eq!(r.skipped.len(), 1);
    }

    #[test]
    fn plan_reply_applies_steps_quadrants_and_names_the_focus_task() {
        let svc = service();
        let existing = svc.handle().mutate(|s| {
            let list = s.lists[0].id.clone();
            s.add_task(Task::new(list, "Old task", 0, TaskSection::Backlog)).id
        });
        let short = &existing[..prompt::SHORT_ID_LEN];
        let reply = format!(
            "{{\"actions\":[\
             {{\"type\":\"create_task\",\"title\":\"Report\",\"quadrant\":\"do_asap\",\"steps\":[\"Outline\",\"Draft\"]}},\
             {{\"type\":\"set_quadrant\",\"task\":\"{short}\",\"quadrant\":\"do_mid_week\"}},\
             {{\"type\":\"start_focus\",\"task\":\"Report\"}}]}}"
        );
        assert_eq!(svc.accept_reply(&reply).len(), 3);
        assert_eq!(svc.pending_quadrants().len(), 1);
        let report = svc.apply_pending(None);
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        svc.handle().read(|s| {
            let report_task = s.tasks.iter().find(|t| t.title == "Report").unwrap();
            assert_eq!(report_task.section, TaskSection::Today);
            assert_eq!(s.steps_of(&report_task.id).len(), 2);
            assert_eq!(report.focus_task.as_deref(), Some(report_task.id.as_str()));
            let old = s.task(&existing).unwrap();
            assert_eq!(old.section, TaskSection::ThisWeek);
        });
    }

    fn ask(svc: &AssistantService, thread: Option<&str>, text: &str) -> (String, Result<TurnOutcome, EngineError>) {
        let id = svc.begin_turn(thread, text).unwrap();
        let out = svc.respond(&id, now(), &mut |_| true);
        (id, out)
    }

    #[test]
    fn begin_turn_validates_and_titles_the_thread() {
        let svc = service();
        assert!(svc.begin_turn(None, "   ").is_err());
        assert!(svc.begin_turn(None, &"x".repeat(MAX_USER_CHARS + 1)).is_err());
        let id = svc.begin_turn(None, "Plan my week please").unwrap();
        let threads = svc.threads();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].title, "Plan my week please");
        let again = svc.begin_turn(Some(&id), "And one more thing").unwrap();
        assert_eq!(again, id);
        assert_eq!(svc.messages(&id).len(), 2);
    }

    #[test]
    fn respond_stores_reply_without_the_action_block() {
        let svc = service();
        let mock = Arc::new(MockEngine::new([
            "Sure.\n```json\n{\"actions\":[{\"type\":\"create_task\",\"title\":\"Write report\"}]}\n```",
        ]));
        svc.set_engine(mock.clone());
        let (id, out) = ask(&svc, None, "add a report task");
        let out = out.unwrap();
        assert_eq!(out.text, "Sure.");
        assert_eq!(out.proposals.len(), 1);
        let msgs = svc.messages(&id);
        assert_eq!(msgs.last().unwrap(), &(false, "Sure.".to_string()));
        // Nothing changed yet.
        assert!(svc.handle().read(|s| s.tasks.is_empty()));
        // The prompt carried the system message and the user's text.
        let prompt = &mock.prompts()[0];
        assert_eq!(prompt[0].role, EngineRole::System);
        assert_eq!(prompt.last().unwrap().content, "add a report task");
    }

    #[test]
    fn nothing_is_applied_until_the_user_agrees() {
        let svc = service();
        svc.set_engine(Arc::new(MockEngine::new([
            "{\"actions\":[{\"type\":\"create_task\",\"title\":\"A\"},{\"type\":\"create_note\",\"title\":\"N\",\"body\":\"b\"}]}",
        ])));
        let (id, out) = ask(&svc, None, "go");
        assert_eq!(out.unwrap().proposals.len(), 2);
        assert_eq!(svc.pending_descriptions().len(), 2);
        assert!(svc.handle().read(|s| s.tasks.is_empty() && s.notes.is_empty()));

        let report = svc.apply_pending(Some(&id));
        assert_eq!(report.applied.len(), 2);
        assert!(report.skipped.is_empty());
        svc.handle().read(|s| {
            assert_eq!(s.tasks.len(), 1);
            assert_eq!(s.tasks[0].title, "A");
            assert_eq!(s.tasks[0].section, TaskSection::Today);
            assert_eq!(s.notes.len(), 1);
            assert_eq!(s.notes[0].body, "b");
        });
        assert!(svc.pending_descriptions().is_empty());
        assert!(svc.messages(&id).last().unwrap().1.starts_with("Done: 2 changes"));
    }

    #[test]
    fn dismissing_discards_the_proposals() {
        let svc = service();
        svc.set_engine(Arc::new(MockEngine::new(["{\"actions\":[{\"type\":\"create_task\",\"title\":\"A\"}]}"])));
        ask(&svc, None, "go").1.unwrap();
        svc.discard_pending();
        assert_eq!(svc.apply_pending(None), ApplyReport::default());
        assert!(svc.handle().read(|s| s.tasks.is_empty()));
    }

    #[test]
    fn a_new_message_discards_stale_proposals() {
        let svc = service();
        svc.set_engine(Arc::new(MockEngine::new(["{\"actions\":[{\"type\":\"create_task\",\"title\":\"A\"}]}"])));
        let (id, _) = ask(&svc, None, "go");
        svc.begin_turn(Some(&id), "never mind").unwrap();
        assert!(svc.pending_descriptions().is_empty());
    }

    #[test]
    fn subtasks_and_schedules_resolve_titles_including_new_tasks() {
        let svc = service();
        let at = Utc.with_ymd_and_hms(2026, 5, 4, 9, 30, 0).unwrap();
        let actions = vec![
            ProposedAction::CreateTask { title: "Write report".into(), section: TaskSection::Today, estimate_mins: 30, list: None, quadrant: None, do_date: None, deadline: None, steps: Vec::new() },
            ProposedAction::AddSubtask { task: "write REPORT".into(), title: "Outline".into() },
            ProposedAction::ScheduleTask { task: "Write report".into(), at },
            ProposedAction::AddSubtask { task: "Ghost task".into(), title: "x".into() },
            ProposedAction::ScheduleTask { task: "Ghost task".into(), at },
        ];
        let report = svc.apply_actions(&actions);
        assert_eq!(report.applied.len(), 3);
        assert_eq!(report.skipped.len(), 2);
        svc.handle().read(|s| {
            let parent = s.tasks.iter().find(|t| t.title == "Write report").unwrap();
            assert_eq!(parent.scheduled_at, Some(at));
            let steps = s.steps_of(&parent.id);
            assert_eq!(steps.len(), 1);
            assert_eq!(steps[0].title, "Outline");
        });
    }

    #[test]
    fn ambiguous_titles_are_skipped_not_guessed() {
        let svc = service();
        svc.apply_actions(&[
            ProposedAction::CreateTask { title: "Dup".into(), section: TaskSection::Today, estimate_mins: 0, list: None, quadrant: None, do_date: None, deadline: None, steps: Vec::new() },
            ProposedAction::CreateTask { title: "Dup".into(), section: TaskSection::Backlog, estimate_mins: 0, list: None, quadrant: None, do_date: None, deadline: None, steps: Vec::new() },
        ]);
        let r = svc.apply_actions(&[ProposedAction::AddSubtask { task: "Dup".into(), title: "x".into() }]);
        assert_eq!(r.applied.len(), 0);
        assert!(r.skipped[0].contains("more than one"));
    }

    #[test]
    fn named_list_is_used_when_it_exists() {
        let svc = service();
        let list_id = svc.handle().mutate(|s| s.add_list(crate::models::TaskList::new("Work", "#3CC9C4")));
        svc.apply_actions(&[
            ProposedAction::CreateTask { title: "In work".into(), section: TaskSection::Today, estimate_mins: 0, list: Some("work".into()), quadrant: None, do_date: None, deadline: None, steps: Vec::new() },
            ProposedAction::CreateTask { title: "Fallback".into(), section: TaskSection::Today, estimate_mins: 0, list: Some("Nope".into()), quadrant: None, do_date: None, deadline: None, steps: Vec::new() },
        ]);
        svc.handle().read(|s| {
            assert_eq!(s.tasks.iter().find(|t| t.title == "In work").unwrap().list_id, list_id);
            assert_ne!(s.tasks.iter().find(|t| t.title == "Fallback").unwrap().list_id, list_id);
        });
    }

    #[test]
    fn hostile_output_changes_nothing() {
        let svc = service();
        svc.set_engine(Arc::new(MockEngine::new([
            "Done! {\"actions\":[{\"type\":\"delete_all\"},{\"type\":\"create_task\",\"title\":\"x\",\"list\":\"a\\u0000b\"}]}",
        ])));
        let (_, out) = ask(&svc, None, "hi");
        assert!(out.unwrap().proposals.is_empty());
        assert!(svc.apply_pending(None).applied.is_empty());
        assert!(svc.handle().read(|s| s.tasks.is_empty()));
    }

    #[test]
    fn engine_failure_surfaces_and_stores_no_reply() {
        let svc = service();
        svc.set_engine(Arc::new(MockEngine::failing(EngineError::Failed("boom".into()))));
        let (id, out) = ask(&svc, None, "hi");
        assert!(out.is_err());
        assert_eq!(svc.messages(&id).len(), 1);
    }

    #[test]
    fn without_a_model_the_status_says_so_and_respond_refuses() {
        let svc = service();
        assert_eq!(svc.model_status(), ModelStatus::NotChosen);
        let id = svc.begin_turn(None, "hi").unwrap();
        assert_eq!(svc.respond(&id, now(), &mut |_| true), Err(EngineError::NotChosen));
    }

    #[test]
    fn memory_add_edit_remove_with_history() {
        let svc = service();
        assert!(svc.add_memory("nonsense", "x").is_err());
        assert!(svc.add_memory("preference", "  ").is_err());
        let id = svc.add_memory("person_or_project", "Sam   leads  design").unwrap();
        assert_eq!(svc.memories()[0].text, "Sam leads design");
        assert_eq!(svc.memories()[0].kind, MemoryKind::PersonOrProject);
        svc.edit_memory(&id, "Sam leads product design").unwrap();
        assert_eq!(svc.memory_history(&id), vec!["Sam leads design".to_string()]);
        assert!(svc.remove_memory(&id));
        assert!(svc.memories().is_empty());
        assert!(!svc.remove_memory(&id));
        assert!(svc.edit_memory(&id, "again").is_err());
    }

    #[test]
    fn memories_reach_the_prompt() {
        let svc = service();
        svc.add_memory("commitment", "Call the dentist on Friday").unwrap();
        let mock = Arc::new(MockEngine::new(["ok"]));
        svc.set_engine(mock.clone());
        ask(&svc, None, "hi").1.unwrap();
        assert!(mock.prompts()[0][0].content.contains("Call the dentist on Friday"));
    }

    #[test]
    fn accepting_or_dismissing_a_suggestion_quiets_it() {
        let svc = service();
        let shown = svc.suggestions(now(), false);
        assert!(!shown.is_empty());
        let id = AssistantService::suggestion_id(shown[0].kind);
        assert!(svc.accept_suggestion(id, now()).is_some());
        assert!(svc.suggestions(now(), false).iter().all(|s| AssistantService::suggestion_id(s.kind) != id));
        assert!(svc.suggestions(now(), true).is_empty());
    }

    #[test]
    fn quick_actions_have_prompts() {
        assert!(AssistantService::quick_action_prompt("quick_prioritize").is_some());
        assert!(AssistantService::quick_action_prompt("wind_down_reflection").is_some());
        assert!(AssistantService::quick_action_prompt("other").is_none());
    }
    #[test]
    fn editing_a_sent_message_keeps_the_old_reply_and_drops_what_followed() {
        let svc = service();
        svc.set_engine(Arc::new(MockEngine::new(["First answer.", "Second answer.", "Third answer."])));
        let (id, _) = ask(&svc, None, "first question");
        let _ = ask(&svc, Some(&id), "follow up");
        assert_eq!(svc.messages(&id).len(), 4);
        // Edit the first question: the follow-up and its reply go, the old reply is kept.
        svc.edit_turn(&id, 0, "better question").unwrap();
        assert_eq!(svc.messages(&id), vec![(true, "better question".to_string())]);
        svc.respond(&id, now(), &mut |_| true).unwrap();
        let views = svc.message_views(&id);
        assert_eq!(views.len(), 2);
        assert_eq!(views[1].text, "Third answer.");
        assert_eq!(views[1].versions, vec!["First answer.".to_string()]);
        // Only sent messages can be edited, and not with empty text.
        assert!(svc.edit_turn(&id, 1, "x").is_err());
        assert!(svc.edit_turn(&id, 0, "  ").is_err());
        assert!(svc.edit_turn("nope", 0, "x").is_err());
    }

    #[test]
    fn threads_can_be_forked_and_deleted() {
        let svc = service();
        svc.set_engine(Arc::new(MockEngine::new(["A.", "B."])));
        let (id, _) = ask(&svc, None, "one");
        let _ = ask(&svc, Some(&id), "two");
        let part = svc.fork_thread(&id, Some(1)).unwrap();
        assert_eq!(svc.messages(&part).len(), 2);
        let whole = svc.fork_thread(&id, None).unwrap();
        assert_eq!(svc.messages(&whole).len(), 4);
        assert_ne!(part, whole);
        assert_eq!(svc.threads().len(), 3);
        assert!(svc.delete_thread(&part));
        assert!(!svc.delete_thread(&part));
        assert_eq!(svc.threads().len(), 2);
        assert_eq!(svc.messages(&id).len(), 4, "the original is untouched");
        assert!(svc.fork_thread("missing", None).is_none());
    }
}
