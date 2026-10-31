//! Wiring for the assistant screen (ui/screens/assistant.slint).
//!
//! Replies are produced on a worker thread so the window never waits on the
//! model. Everything else is a quick service call on the UI thread.

use std::thread;

use chrono::{DateTime, Local, Utc};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use thulli_core::services::assistant::Suggestion;
use thulli_core::services::{AssistantService, TimerPhase};

use crate::app::context::AppContext;
use crate::{AssistantState, ChatMessageModel, ChatThreadModel, MainWindow, MemoryModel, SuggestionModel};

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let state = window.global::<AssistantState>();
    refresh(window, ctx);

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_send(move |text| {
            if let Some(w) = weak.upgrade() {
                send(&w, &ctx, text.as_str());
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_new_chat(move || {
            if let Some(w) = weak.upgrade() {
                let id = ctx.assistant.new_thread();
                let state = w.global::<AssistantState>();
                state.set_open_thread_id(id.into());
                state.set_status_text(SharedString::new());
                refresh(&w, &ctx);
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_open_thread(move |id| {
            if let Some(w) = weak.upgrade() {
                // Proposals belong to the thread they came from.
                ctx.assistant.discard_pending();
                let state = w.global::<AssistantState>();
                state.set_open_thread_id(id);
                state.set_status_text(SharedString::new());
                refresh(&w, &ctx);
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_add_memory(move |kind, text| {
            if let Some(w) = weak.upgrade() {
                report_error(&w, ctx.assistant.add_memory(kind.as_str(), text.as_str()).err());
                refresh(&w, &ctx);
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_edit_memory(move |id, text| {
            if let Some(w) = weak.upgrade() {
                report_error(&w, ctx.assistant.edit_memory(id.as_str(), text.as_str()).err());
                refresh(&w, &ctx);
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_archive_memory(move |id| {
            if let Some(w) = weak.upgrade() {
                ctx.assistant.remove_memory(id.as_str());
                let state = w.global::<AssistantState>();
                if state.get_history_memory_id() == id {
                    state.set_history_memory_id(SharedString::new());
                }
                refresh(&w, &ctx);
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_show_memory_history(move |id| {
            if let Some(w) = weak.upgrade() {
                let earlier = ctx.assistant.memory_history(id.as_str());
                let state = w.global::<AssistantState>();
                state.set_history_memory_id(id);
                state.set_memory_history(string_model(earlier));
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_accept_suggestion(move |id| {
            if let Some(w) = weak.upgrade() {
                let now = Local::now();
                match ctx.assistant.accept_suggestion(id.as_str(), now) {
                    Some(prompt) => send(&w, &ctx, prompt),
                    None => refresh(&w, &ctx),
                }
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_dismiss_suggestion(move |id| {
            if let Some(w) = weak.upgrade() {
                ctx.assistant.dismiss_suggestion(id.as_str(), Local::now());
                refresh(&w, &ctx);
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_quick_action(move |id| {
            if let Some(w) = weak.upgrade() {
                match AssistantService::quick_action_prompt(id.as_str()) {
                    Some(prompt) => send(&w, &ctx, prompt),
                    None => refresh(&w, &ctx),
                }
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_apply_proposals(move || {
            if let Some(w) = weak.upgrade() {
                let open = open_thread(&w);
                let report = ctx.assistant.apply_pending(open.as_deref());
                w.global::<AssistantState>().set_status_text(report.summary().into());
                refresh(&w, &ctx);
            }
        });
    }

    {
        let weak = window.as_weak();
        let ctx = ctx.clone();
        state.on_discard_proposals(move || {
            if let Some(w) = weak.upgrade() {
                ctx.assistant.discard_pending();
                refresh(&w, &ctx);
            }
        });
    }
}

/// Sends a user message and asks the model for a reply on a worker thread.
fn send(window: &MainWindow, ctx: &AppContext, text: &str) {
    let state = window.global::<AssistantState>();
    if state.get_busy() {
        return;
    }
    let svc = ctx.assistant.clone();
    let status = svc.model_status();
    if !status.is_ready() {
        state.set_status_text(status.notice().into());
        refresh(window, ctx);
        return;
    }
    let open = open_thread(window);
    let thread_id = match svc.begin_turn(open.as_deref(), text) {
        Ok(id) => id,
        Err(why) => {
            state.set_status_text(why.into());
            return;
        }
    };
    state.set_open_thread_id(thread_id.clone().into());
    state.set_status_text(SharedString::new());
    state.set_busy(true);
    refresh(window, ctx);

    let focus_active = focus_running(ctx);
    let weak = window.as_weak();
    let ui_svc = svc.clone();
    thread::spawn(move || {
        let outcome = svc.respond(&thread_id, Local::now(), &mut |_| true);
        slint::invoke_from_event_loop(move || {
            let Some(w) = weak.upgrade() else { return };
            let state = w.global::<AssistantState>();
            state.set_busy(false);
            if let Err(why) = outcome {
                state.set_status_text(why.user_message().into());
            }
            refresh_with(&w, &ui_svc, focus_active);
        })
        .ok();
    });
}

/// Rebuilds every list and flag on the assistant screen from the services.
fn refresh(window: &MainWindow, ctx: &AppContext) {
    refresh_with(window, &ctx.assistant, focus_running(ctx));
}

fn refresh_with(window: &MainWindow, svc: &AssistantService, focus_active: bool) {
    let state = window.global::<AssistantState>();
    let now = Local::now();

    state.set_threads(ModelRc::new(VecModel::from(
        svc.threads()
            .into_iter()
            .map(|t| ChatThreadModel {
                id: t.id.into(),
                title: t.title.into(),
                updated_label: local_label(t.updated_at).into(),
            })
            .collect::<Vec<_>>(),
    )));

    let open = state.get_open_thread_id();
    let messages: Vec<ChatMessageModel> = if open.is_empty() {
        Vec::new()
    } else {
        svc.messages(open.as_str())
            .into_iter()
            .enumerate()
            .map(|(i, (from_user, text))| ChatMessageModel {
                id: format!("{i}").into(),
                from_user,
                text: text.into(),
            })
            .collect()
    };
    state.set_messages(ModelRc::new(VecModel::from(messages)));

    state.set_memories(ModelRc::new(VecModel::from(
        svc.memories()
            .into_iter()
            .map(|m| MemoryModel {
                id: m.id.into(),
                kind: m.kind.as_str().into(),
                text: m.text.into(),
                updated_label: local_label(m.updated_at).into(),
            })
            .collect::<Vec<_>>(),
    )));

    state.set_suggestions(ModelRc::new(VecModel::from(
        svc.suggestions(now, focus_active).into_iter().map(suggestion_model).collect::<Vec<_>>(),
    )));

    state.set_day_summary(svc.day_summary(now.date_naive()).into());
    state.set_proposals(string_model(svc.pending_descriptions()));

    let status = svc.model_status();
    state.set_model_ready(status.is_ready());
    state.set_model_notice(status.notice().into());
}

fn suggestion_model(s: Suggestion) -> SuggestionModel {
    SuggestionModel {
        id: AssistantService::suggestion_id(s.kind).into(),
        kind: AssistantService::suggestion_id(s.kind).into(),
        title: s.title.into(),
        body: s.body.into(),
    }
}

fn report_error(window: &MainWindow, err: Option<String>) {
    if let Some(why) = err {
        window.global::<AssistantState>().set_status_text(why.into());
    }
}

fn open_thread(window: &MainWindow) -> Option<String> {
    let id = window.global::<AssistantState>().get_open_thread_id();
    if id.is_empty() { None } else { Some(id.to_string()) }
}

fn focus_running(ctx: &AppContext) -> bool {
    ctx.timer.snapshot().phase == TimerPhase::Running
}

fn string_model(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

fn local_label(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%d %b, %H:%M").to_string()
}
