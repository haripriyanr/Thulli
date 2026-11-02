//! Connects the screens to the saved data. Every change goes through a
//! service, which saves it; then the affected screen properties are rebuilt
//! from the saved state.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use chrono::{Local, Utc};
use slint::ComponentHandle;
use thulli_core::models::{AppStateStore, TaskList, TaskSection, TimeEntry, TimeEntryKind};
use thulli_core::repository::StorageRepository;
use thulli_core::services::notes::NoteScope;
use thulli_core::services::pomodoro::{PomodoroPhase, PomodoroSettings, PomodoroTimer};
use thulli_core::services::{NotesService, PreferencesService, StoreHandle, TaskService};

use crate::convert::{self, model};
use crate::{AppData, MainWindow, NoteRow};

/// Setting keys the Settings screen can step, with the step size for each.
const STEPPED_SETTINGS: [(&str, i32); 4] = [
    ("default_session_mins", 5),
    ("default_break_mins", 1),
    ("pomodoro_long_break_mins", 1),
    ("pomodoro_rounds", 1),
];

pub struct DataHub {
    handle: StoreHandle,
    tasks: TaskService,
    notes: NotesService,
    prefs: PreferencesService,
    pomodoro: RefCell<PomodoroTimer>,
    selected_note: RefCell<Option<String>>,
}

impl DataHub {
    pub fn new(state: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Rc<Self> {
        let handle = StoreHandle::new(state.clone(), repository.clone());
        let settings = PomodoroSettings::from_preferences(&handle.read(|s| s.preferences.clone()));
        Rc::new(Self {
            tasks: TaskService::new(state, repository),
            notes: NotesService::new(handle.clone()),
            prefs: PreferencesService::new(handle.clone()),
            handle,
            pomodoro: RefCell::new(PomodoroTimer::new(settings)),
            selected_note: RefCell::new(None),
        })
    }

    /// Fills every screen from the saved state and registers the callbacks.
    pub fn attach(self: &Rc<Self>, window: &MainWindow) {
        self.refresh_all(window);
        self.wire_pomodoro(window);
        self.wire_tasks(window);
        self.wire_notes(window);
        self.wire_settings(window);
    }

    /// Advances the Pomodoro timer by one second and refreshes the window.
    pub fn tick_pomodoro(&self, window: &MainWindow) {
        let mut timer = self.pomodoro.borrow_mut();
        let before = timer.elapsed_in_phase();
        let phase = timer.phase();
        if timer.tick().is_some() {
            drop(timer);
            self.record_time(phase, before + 1);
            self.refresh_all(window);
            return;
        }
        drop(timer);
        self.refresh_pomodoro(window);
    }

    pub fn refresh_all(&self, window: &MainWindow) {
        self.refresh_pomodoro(window);
        self.refresh_tasks(window);
        self.refresh_notes(window);
        self.refresh_insights(window);
        self.refresh_settings(window);
    }

    fn refresh_pomodoro(&self, window: &MainWindow) {
        let timer = self.pomodoro.borrow();
        let data = window.global::<AppData>();
        data.set_pomodoro_time(timer.display().into());
        data.set_pomodoro_progress(timer.progress());
        data.set_pomodoro_running(timer.is_running());
        data.set_pomodoro_phase(timer.phase().as_str().into());
        data.set_pomodoro_round(
            format!("Round {} of {}", timer.round_number(), timer.rounds_before_long_break()).into(),
        );
        data.set_pomodoro_status(pomodoro_status(&timer).into());
        let prefs = self.prefs.snapshot();
        data.set_pomodoro_durations(
            format!(
                "Focus {} min  ·  Short break {} min  ·  Long break {} min",
                prefs.default_session_mins, prefs.default_break_mins, prefs.pomodoro_long_break_mins
            )
            .into(),
        );
    }

    fn refresh_tasks(&self, window: &MainWindow) {
        let data = window.global::<AppData>();
        let (today_rows, week_rows, backlog_rows, done_rows) = self.handle.read(|s| {
            let open = |section: TaskSection| {
                convert::task_rows(s, move |t| !t.is_completed() && t.section == section)
            };
            (
                open(TaskSection::Today),
                open(TaskSection::ThisWeek),
                open(TaskSection::Backlog),
                convert::task_rows(s, |t| t.is_completed()),
            )
        });
        data.set_tasks_today(model(today_rows.clone()));
        data.set_tasks_week(model(week_rows));
        data.set_tasks_backlog(model(backlog_rows));
        data.set_tasks_done(model(done_rows));
        data.set_today_tasks(model(today_rows));
    }

    fn refresh_notes(&self, window: &MainWindow) {
        let data = window.global::<AppData>();
        // notes_in takes the store lock itself, so it must run outside any read.
        let rows: Vec<NoteRow> = self.notes.notes_in(&NoteScope::All).iter().map(convert::note_row).collect();
        let selected = self.selected_note.borrow().clone();
        data.set_notes(model(rows));
        if let Some(id) = selected {
            if let Some(note) = self.notes.note(&id) {
                data.set_selected_note_id(note.id.clone().into());
                data.set_selected_note_title(note.title.clone().into());
                data.set_selected_note_body(note.body.clone().into());
            } else {
                *self.selected_note.borrow_mut() = None;
                data.set_selected_note_id("".into());
                data.set_selected_note_title("".into());
                data.set_selected_note_body("".into());
            }
        }
    }

    fn refresh_insights(&self, window: &MainWindow) {
        let today = Local::now().date_naive();
        let data = window.global::<AppData>();
        let (focus_today, focus_week, sessions, streak, bars) = self.handle.read(|s| {
            (
                convert::focus_secs_on(s, today),
                convert::focus_secs_this_week(s, today),
                convert::focus_sessions_this_week(s, today),
                convert::focus_streak(s, today),
                convert::week_bars(s, today),
            )
        });
        data.set_today_focus(thulli_core::services::insights::format_duration(focus_today).into());
        data.set_insight_focus_week(thulli_core::services::insights::format_duration(focus_week).into());
        data.set_insight_sessions(sessions.to_string().into());
        data.set_insight_streak(format!("{streak} days").into());
        data.set_insight_bars(model(bars));
    }

    fn refresh_settings(&self, window: &MainWindow) {
        let prefs = self.prefs.snapshot();
        let data = window.global::<AppData>();
        data.set_setting_focus_mins(prefs.default_session_mins as i32);
        data.set_setting_short_mins(prefs.default_break_mins as i32);
        data.set_setting_long_mins(prefs.pomodoro_long_break_mins as i32);
        data.set_setting_rounds(prefs.pomodoro_rounds as i32);
        data.set_setting_auto_advance(prefs.pomodoro_auto_advance);
    }

    /// Adds the elapsed time of a phase to the saved time entries.
    fn record_time(&self, phase: PomodoroPhase, secs: u64) {
        if secs == 0 {
            return;
        }
        let kind = match phase {
            PomodoroPhase::Focus => TimeEntryKind::Focus,
            _ => TimeEntryKind::Break,
        };
        let ended = Utc::now();
        let started = ended - chrono::Duration::seconds(secs as i64);
        self.handle.mutate(|s| s.time_entries.push(TimeEntry::new(kind, started, ended)));
    }

    fn apply_pomodoro_settings(&self) {
        let settings = PomodoroSettings::from_preferences(&self.prefs.snapshot());
        self.pomodoro.borrow_mut().configure(settings);
    }

    fn wire_pomodoro(self: &Rc<Self>, window: &MainWindow) {
        let data = window.global::<AppData>();
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_pomodoro_toggle(move || {
                hub.pomodoro.borrow_mut().toggle();
                if let Some(w) = weak.upgrade() {
                    hub.refresh_pomodoro(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_pomodoro_next(move || {
                let mut timer = hub.pomodoro.borrow_mut();
                let (phase, elapsed) = (timer.phase(), timer.elapsed_in_phase());
                timer.next();
                drop(timer);
                hub.record_time(phase, elapsed);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_pomodoro_reset(move || {
                let mut timer = hub.pomodoro.borrow_mut();
                let (phase, elapsed) = (timer.phase(), timer.elapsed_in_phase());
                timer.reset();
                drop(timer);
                hub.record_time(phase, elapsed);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_pomodoro_select(move |id| {
                let Some(target) = PomodoroPhase::from_str_id(id.as_str()) else {
                    return;
                };
                let mut timer = hub.pomodoro.borrow_mut();
                let (phase, elapsed) = (timer.phase(), timer.elapsed_in_phase());
                timer.select(target);
                drop(timer);
                hub.record_time(phase, elapsed);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
    }

    fn default_list_id(&self) -> String {
        self.handle.mutate(|s| {
            if let Some(list) = s.lists.iter().find(|list| !list.is_archived) {
                return list.id.clone();
            }
            let list = TaskList::new_personal();
            let id = list.id.clone();
            s.lists.push(list);
            id
        })
    }

    fn wire_tasks(self: &Rc<Self>, window: &MainWindow) {
        let data = window.global::<AppData>();
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_add_task(move |text| {
                let text = text.trim().to_string();
                if !text.is_empty() {
                    let list_id = hub.default_list_id();
                    hub.tasks.add_smart_task(&text, &list_id, TaskSection::Today);
                }
                if let Some(w) = weak.upgrade() {
                    w.global::<AppData>().set_new_task_text("".into());
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_toggle_task(move |id| {
                hub.tasks.toggle_task(id.as_str());
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_move_task(move |id, section| {
                if let Some(section) = convert::section_from_id(section.as_str()) {
                    hub.tasks.move_to_section(id.as_str(), section);
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_delete_task(move |id| {
                hub.tasks.delete_task(id.as_str());
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
    }

    fn wire_notes(self: &Rc<Self>, window: &MainWindow) {
        let data = window.global::<AppData>();
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_new_note(move || {
                let note = hub.notes.create_note(None);
                *hub.selected_note.borrow_mut() = Some(note.id.clone());
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_select_note(move |id| {
                *hub.selected_note.borrow_mut() = Some(id.to_string());
                if let Some(w) = weak.upgrade() {
                    hub.refresh_notes(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_save_note(move |title, body| {
                let id = hub.selected_note.borrow().clone();
                if let Some(id) = id {
                    hub.notes.update_note(&id, title.as_str(), body.as_str());
                    if let Some(w) = weak.upgrade() {
                        hub.refresh_notes(&w);
                    }
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_delete_note(move || {
                let id = hub.selected_note.borrow_mut().take();
                if let Some(id) = id {
                    hub.notes.trash_note(&id);
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
    }

    fn wire_settings(self: &Rc<Self>, window: &MainWindow) {
        let data = window.global::<AppData>();
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_adjust_setting(move |key, direction| {
                let key = key.to_string();
                if let Some((_, step)) = STEPPED_SETTINGS.iter().find(|(k, _)| *k == key) {
                    let current = hub.prefs.snapshot();
                    let value = match key.as_str() {
                        "default_session_mins" => current.default_session_mins,
                        "default_break_mins" => current.default_break_mins,
                        "pomodoro_long_break_mins" => current.pomodoro_long_break_mins,
                        _ => current.pomodoro_rounds,
                    } as i32;
                    let _ = hub.prefs.set_int(&key, value + direction * step);
                    hub.apply_pomodoro_settings();
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_set_auto_advance(move |on| {
                let _ = hub.prefs.set_bool("pomodoro_auto_advance", on);
                hub.apply_pomodoro_settings();
                if let Some(w) = weak.upgrade() {
                    hub.refresh_settings(&w);
                }
            });
        }
    }
}

fn pomodoro_status(timer: &PomodoroTimer) -> &'static str {
    let untouched = timer.remaining_secs() == timer.phase_total_secs();
    match (timer.phase(), timer.is_running(), untouched) {
        (PomodoroPhase::Focus, true, _) => "Focusing",
        (PomodoroPhase::Focus, false, true) => "Ready to focus",
        (PomodoroPhase::Focus, false, false) => "Paused",
        (_, true, _) => "Break time",
        (_, false, true) => "Ready for a break",
        (_, false, false) => "Paused",
    }
}
