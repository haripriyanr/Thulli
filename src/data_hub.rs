//! Connects the screens to the saved data. Every change goes through a
//! service, which saves it; then the affected screen properties are rebuilt
//! from the saved state.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use chrono::{Local, NaiveDate, Utc};
use slint::ComponentHandle;
use thulli_core::models::{AppStateStore, TaskSection, TaskStatus, TimeEntry, TimeEntryKind};
use thulli_core::repository::StorageRepository;
use thulli_core::services::notes::NoteScope;
use thulli_core::services::pomodoro::{PomodoroPhase, PomodoroSettings, PomodoroTimer};
use thulli_core::models::MatrixQuadrant;
use thulli_core::services::preferences::{language_name, LANGUAGE_CHOICES};
use thulli_core::services::tasks::{parse_date_input, DateFilter, TaskEdit};
use thulli_core::services::{ActionHistory, ListService, NotesService, PreferencesService, StoreHandle, TaskService};

use crate::audio::{Ambient, AmbientPlayer};
use chrono::Duration as ChronoDuration;
use thulli_core::services::insights::format_duration;
use crate::convert::{self, model};
use crate::spaces;
use thulli_core::services::assistant::AssistantService;

#[path = "data_hub_ai.rs"]
mod ai;
use crate::{AppData, LanguageRow, MainWindow, NoteRow};

/// Setting keys the Settings screen can step, with the step size for each.
const STEPPED_SETTINGS: [(&str, i32); 5] = [
    ("default_session_mins", 5),
    ("default_break_mins", 1),
    ("pomodoro_long_break_mins", 1),
    ("pomodoro_rounds", 1),
    ("pomodoro_custom_mins", 5),
];

/// Label for focus with no task and no typed name.
const GENERAL_FOCUS: &str = "General Focus";

pub struct DataHub {
    handle: StoreHandle,
    tasks: TaskService,
    notes: NotesService,
    prefs: PreferencesService,
    pomodoro: RefCell<PomodoroTimer>,
    selected_note: RefCell<Option<String>>,
    /// "focus" or "custom": which focus interval the timer is using.
    mode: RefCell<String>,
    /// "day", "week", "month" or "year": the range of the analytics panel.
    activity_range: RefCell<String>,
    /// Plays the ambient sound while the timer runs.
    audio: RefCell<AmbientPlayer>,
    /// The name typed for the current focus round. Not saved; cleared on
    /// reopen and when a focus round finishes.
    session_name: RefCell<String>,
    /// The task the session is linked to. None means a freeform session.
    task_choice: RefCell<Option<String>>,
    /// Search text in the task picker.
    task_search: RefCell<String>,
    /// Spaces are saved task lists.
    lists: ListService,
    /// Undo and redo for changes made on the Spaces screen.
    history: ActionHistory,
    /// The space shown on the Spaces screen; empty means every space.
    space: RefCell<String>,
    /// A day inside the calendar week on show.
    calendar_anchor: RefCell<NaiveDate>,
    /// Plans and sorts with a model; its proposals wait for the user.
    assistant: AssistantService,
    /// The Sort screen's queue and AI suggestions.
    triage: RefCell<ai::Triage>,
    /// On-device models, loaded on first use and shared with worker threads.
    models: Arc<thulli_core::ai::Models>,
    /// The microphone while a mic button is on, with the field it serves.
    recorder: RefCell<Option<(String, crate::voice::Recorder)>>,
}

impl DataHub {
    pub fn new(state: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Rc<Self> {
        let handle = StoreHandle::new(state.clone(), repository.clone());
        let settings = PomodoroSettings::from_preferences(&handle.read(|s| s.preferences.clone()));
        Rc::new(Self {
            lists: ListService::new(state.clone(), repository.clone()),
            history: ActionHistory::new(handle.clone()),
            space: RefCell::new(String::new()),
            calendar_anchor: RefCell::new(Local::now().date_naive()),
            assistant: AssistantService::new(handle.clone()),
            triage: RefCell::new(ai::Triage::default()),
            models: Arc::new(thulli_core::ai::Models::default()),
            recorder: RefCell::new(None),
            tasks: TaskService::new(state, repository),
            notes: NotesService::new(handle.clone()),
            prefs: PreferencesService::new(handle.clone()),
            handle,
            pomodoro: RefCell::new(PomodoroTimer::new(settings)),
            selected_note: RefCell::new(None),
            mode: RefCell::new("focus".to_string()),
            activity_range: RefCell::new("week".to_string()),
            audio: RefCell::new(AmbientPlayer::default()),
            session_name: RefCell::new(String::new()),
            task_choice: RefCell::new(None),
            task_search: RefCell::new(String::new()),
        })
    }

    /// Fills every screen from the saved state and registers the callbacks.
    pub fn attach(self: &Rc<Self>, window: &MainWindow) {
        self.refresh_all(window);
        self.refresh_ai(window);
        self.wire_ai(window);
        self.wire_pomodoro(window);
        self.wire_tasks(window);
        self.wire_spaces(window);
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
            self.record_time(phase, before + 1, true);
            if phase == PomodoroPhase::Focus {
                self.session_name.borrow_mut().clear();
                window.global::<AppData>().set_session_name("".into());
            }
            self.refresh_all(window);
            return;
        }
        drop(timer);
        self.refresh_pomodoro(window);
    }

    pub fn refresh_all(&self, window: &MainWindow) {
        self.refresh_pomodoro(window);
        self.refresh_tasks(window);
        self.refresh_spaces(window);
        self.refresh_notes(window);
        self.refresh_insights(window);
        self.refresh_settings(window);
        self.refresh_activity(window);
        self.refresh_task_picker(window);
    }

    fn refresh_pomodoro(&self, window: &MainWindow) {
        let timer = self.pomodoro.borrow();
        let data = window.global::<AppData>();
        data.set_pomodoro_time(timer.display().into());
        data.set_pomodoro_progress(timer.progress());
        data.set_pomodoro_running(timer.is_running());
        data.set_pomodoro_phase(timer.phase().as_str().into());
        data.set_pomodoro_mode(display_mode(timer.phase(), &self.mode.borrow()).into());
        data.set_pomodoro_round(
            format!("Round {} of {}", timer.round_number(), timer.rounds_before_long_break()).into(),
        );
        data.set_pomodoro_status(pomodoro_status(&timer).into());
        let prefs = self.prefs.snapshot();
        data.set_pomodoro_custom_mins(prefs.pomodoro_custom_mins as i32);
        data.set_pomodoro_rounds_done(timer.completed_rounds() as i32);
        data.set_ambient_choice(prefs.pomodoro_ambient.clone().into());
        let choice = Ambient::from_id(&prefs.pomodoro_ambient).unwrap_or(Ambient::Forest);
        self.audio.borrow_mut().sync(choice, timer.is_running());
        data.set_pomodoro_durations(
            format!(
                "Focus {} min  ·  Short break {} min  ·  Long break {} min",
                prefs.default_session_mins, prefs.default_break_mins, prefs.pomodoro_long_break_mins
            )
            .into(),
        );
    }

    fn refresh_activity(&self, window: &MainWindow) {
        let today = Local::now().date_naive();
        let range = self.activity_range.borrow().clone();
        // The previous period is the same window shifted back by its own length.
        let shift = match range.as_str() {
            "day" => ChronoDuration::days(1),
            "month" => ChronoDuration::days(30),
            "year" => ChronoDuration::days(365),
            _ => ChronoDuration::days(7),
        };
        let (activity, previous, streak) = self.handle.read(|s| {
            (
                convert::activity(s, today, &range),
                convert::activity(s, today - shift, &range),
                convert::focus_streak(s, today),
            )
        });
        let data = window.global::<AppData>();
        data.set_activity_range(range.into());
        data.set_activity_sessions(activity.sessions.to_string().into());
        data.set_activity_sessions_change(change(activity.sessions as u64, previous.sessions as u64).into());
        data.set_activity_focus(format_duration(activity.focus_secs).into());
        data.set_activity_focus_change(change(activity.focus_secs, previous.focus_secs).into());
        let average = if activity.sessions == 0 { 0 } else { activity.focus_secs / activity.sessions as u64 };
        data.set_activity_average(format_duration(average).into());
        data.set_activity_best(activity.best.into());
        data.set_activity_axis_max(activity.axis_max as i32);
        data.set_activity_streak(format!("{streak} days").into());
        data.set_activity_bars(model(activity.bars));
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

    fn refresh_spaces(&self, window: &MainWindow) {
        let data = window.global::<AppData>();
        let today = Local::now().date_naive();
        let rows = self.handle.read(spaces::space_rows);
        // A deleted or archived space falls back to every space.
        let mut space = self.space.borrow().clone();
        if !space.is_empty() && !rows.iter().any(|r| r.id.as_str() == space) {
            space.clear();
            self.space.borrow_mut().clear();
        }
        match rows.iter().find(|r| r.id.as_str() == space) {
            Some(row) => {
                data.set_space_name(row.name.clone());
                data.set_space_color(row.color);
                data.set_space_letter(row.letter.clone());
                data.set_space_open_count(row.count);
            }
            None => {
                data.set_space_name("All spaces".into());
                data.set_space_color(spaces::hex_color("#A78BFA"));
                data.set_space_letter("A".into());
                data.set_space_open_count(rows.iter().map(|r| r.count).sum());
            }
        }
        data.set_all_open_count(rows.iter().map(|r| r.count).sum());
        data.set_space_id(space.clone().into());
        data.set_spaces(model(rows));

        let board_view = self.tasks.board(&spaces::board_scope(&space), DateFilter::Any);
        let anchor = *self.calendar_anchor.borrow();
        let (board, (quadrants, unsorted, unsorted_count), (label, days, unscheduled)) = self.handle.read(|s| {
            (
                spaces::board(s, &board_view),
                spaces::matrix_columns(s, &space),
                spaces::calendar_week(s, &space, anchor, today),
            )
        });
        data.set_board_columns(model(board));
        data.set_matrix_quadrants(model(quadrants));
        data.set_matrix_unsorted(model(unsorted));
        data.set_matrix_unsorted_count(unsorted_count as i32);
        data.set_calendar_label(label.into());
        data.set_calendar_days(model(days));
        data.set_calendar_unscheduled(model(unscheduled));
        data.set_can_undo(self.history.can_undo());
        data.set_can_redo(self.history.can_redo());
    }

    /// The space new tasks go into: the one on show, or the first space.
    fn target_space(&self) -> String {
        let space = self.space.borrow().clone();
        if space.is_empty() { self.default_list_id() } else { space }
    }

    fn wire_spaces(self: &Rc<Self>, window: &MainWindow) {
        let data = window.global::<AppData>();
        // Each handler changes data through a service, then refreshes.
        let on = |f: Box<dyn Fn(&Rc<Self>)>| {
            let hub = self.clone();
            let weak = window.as_weak();
            move || {
                f(&hub);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            }
        };
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_selected(move |id| {
                *hub.space.borrow_mut() = id.to_string();
                if let Some(w) = weak.upgrade() {
                    hub.refresh_spaces(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_created(move |name, color| {
                if let Ok(list) = hub.lists.create_list_with_icon(&name, &color, "") {
                    *hub.space.borrow_mut() = list.id;
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_edited(move |id, name, color| {
                let _ = hub.lists.edit_list(&id, &name, &color, thulli_core::services::lists::IconChange::Keep);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_deleted(move |id| {
                hub.lists.delete_list(&id);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_task_added(move |section, text| {
                let section = spaces::section(&section).unwrap_or(TaskSection::Today);
                let space = hub.target_space();
                hub.tasks.add_task_recorded(&hub.history, &text, &space, section);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_task_composed(move |section, title, estimate, space, do_date, notes| {
                let section = spaces::section(&section).unwrap_or(TaskSection::Today);
                let space = if space.is_empty() { hub.target_space() } else { space.to_string() };
                if let Some(task) = hub.tasks.add_task_recorded(&hub.history, &title, &space, section) {
                    let today = Local::now().date_naive();
                    let edit = TaskEdit {
                        title: task.title.clone(),
                        notes: notes.to_string(),
                        // A picked estimate wins over one typed in the title.
                        estimate_mins: if estimate > 0 { estimate as u32 } else { task.estimate_mins },
                        tag_ids: task.tags.clone(),
                        do_date: parse_date_input(&do_date, today).unwrap_or(None),
                        deadline: task.deadline,
                    };
                    hub.tasks.update_task(&hub.history, &task.id, edit);
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_task_moved(move |id, section| {
                let done = hub.handle.read(|s| s.task(&id).is_some_and(|t| t.is_completed()));
                if section.as_str() == "done" {
                    // Dropped on Done: finish it.
                    if !done {
                        hub.tasks.set_status_recorded(&hub.history, &id, TaskStatus::Completed);
                    }
                } else if let Some(section) = spaces::section(&section) {
                    // A finished task dragged back out is open again.
                    if done {
                        hub.tasks.set_status_recorded(&hub.history, &id, TaskStatus::Open);
                    }
                    hub.tasks.move_to_section_recorded(&hub.history, &id, section);
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_task_quadrant_set(move |id, quadrant| {
                hub.tasks.set_quadrant(&hub.history, &id, MatrixQuadrant::parse(&quadrant));
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_task_day_set(move |id, index| {
                let anchor = *hub.calendar_anchor.borrow();
                let day = (index >= 0).then(|| hub.handle.read(|s| spaces::week_day(s, anchor, index)));
                hub.tasks.set_do_date(&hub.history, &id, day);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_calendar_task_added(move |index, text| {
                let anchor = *hub.calendar_anchor.borrow();
                let day = hub.handle.read(|s| spaces::week_day(s, anchor, index));
                let space = hub.target_space();
                if let Some(task) = hub.tasks.add_task_recorded(&hub.history, &text, &space, TaskSection::ThisWeek) {
                    hub.tasks.set_do_date(&hub.history, &task.id, Some(day));
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_calendar_shift(move |delta| {
                let today = Local::now().date_naive();
                let mut anchor = hub.calendar_anchor.borrow_mut();
                *anchor = if delta == 0 { today } else { *anchor + ChronoDuration::days(7 * delta as i64) };
                drop(anchor);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_spaces(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_task_edited(move |id, title, estimate, notes, do_date, deadline| {
                let today = Local::now().date_naive();
                let (current, tags) = match hub.handle.read(|s| s.task(&id).map(|t| (t.clone(), t.tags.clone()))) {
                    Some(found) => found,
                    None => return,
                };
                // A date that cannot be read keeps the saved one.
                let date = |text: &str, saved| parse_date_input(text, today).unwrap_or(saved);
                let edit = TaskEdit {
                    title: title.to_string(),
                    notes: notes.to_string(),
                    estimate_mins: estimate.trim().parse().unwrap_or(current.estimate_mins),
                    tag_ids: tags,
                    do_date: date(&do_date, current.do_date),
                    deadline: date(&deadline, current.deadline),
                };
                hub.tasks.update_task(&hub.history, &id, edit);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_task_toggled(move |id| {
                hub.tasks.toggle_complete(&hub.history, &id);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_space_task_deleted(move |id| {
                hub.tasks.delete_recorded(&hub.history, &id);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_step_added(move |parent, title| {
                hub.tasks.add_step(&hub.history, &parent, &title);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_task_launched(move |id| {
                let Some(w) = weak.upgrade() else {
                    return;
                };
                // The task moves to Today, the session links to it and a
                // fresh focus round starts at once.
                hub.tasks.start_focus(&id);
                *hub.task_choice.borrow_mut() = Some(id.to_string());
                *hub.mode.borrow_mut() = "focus".to_string();
                hub.switch_phase(&w, PomodoroPhase::Focus);
                hub.pomodoro.borrow_mut().start();
                hub.refresh_all(&w);
            });
        }
        data.on_space_undo(on(Box::new(|hub| {
            hub.history.undo();
        })));
        data.on_space_redo(on(Box::new(|hub| {
            hub.history.redo();
        })));
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
        data.set_session_history(model(self.handle.read(|s| convert::recent_sessions(s, 8))));
    }

    fn refresh_settings(&self, window: &MainWindow) {
        let prefs = self.prefs.snapshot();
        let data = window.global::<AppData>();
        data.set_setting_focus_mins(prefs.default_session_mins as i32);
        data.set_setting_short_mins(prefs.default_break_mins as i32);
        data.set_setting_long_mins(prefs.pomodoro_long_break_mins as i32);
        data.set_setting_rounds(prefs.pomodoro_rounds as i32);
        data.set_setting_auto_advance(prefs.pomodoro_auto_advance);
        data.set_setting_breaks(prefs.pomodoro_breaks);
        data.set_setting_start_next(prefs.start_next_after_early_break);
        data.set_setting_language(prefs.language.clone().into());
        data.set_language_name(language_name(&prefs.language).into());
        data.set_language_rows(model(language_rows("")));
    }

    /// Adds the elapsed time of a phase to the saved time entries.
    /// Saves `secs` of a phase as a time entry. A focus session linked to a
    /// task is labelled with the task's title and carries its id. Otherwise it
    /// takes the typed name, or General Focus when nothing was typed.
    fn record_time(&self, phase: PomodoroPhase, secs: u64, completed: bool) {
        if secs == 0 {
            return;
        }
        let kind = match phase {
            PomodoroPhase::Focus => TimeEntryKind::Focus,
            _ => TimeEntryKind::Break,
        };
        let ended = Utc::now();
        let started = ended - chrono::Duration::seconds(secs as i64);
        let mut entry = TimeEntry::new(kind, started, ended);
        entry.completed = completed;
        if kind == TimeEntryKind::Focus {
            let task = self.task_choice.borrow().clone().and_then(|id| {
                self.handle.read(|s| {
                    s.tasks.iter().find(|t| t.id == id).map(|t| (t.id.clone(), t.title.clone()))
                })
            });
            let typed = self.session_name.borrow().trim().to_string();
            match task {
                Some((id, title)) => {
                    entry.task_id = Some(id);
                    entry.label = title;
                }
                None => {
                    entry.label = if typed.is_empty() { GENERAL_FOCUS.to_string() } else { typed };
                }
            }
        }
        self.handle.mutate(|s| s.time_entries.push(entry));
    }

    /// Fills the task picker and the task shown on the timer card.
    fn refresh_task_picker(&self, window: &MainWindow) {
        let today = Local::now().date_naive();
        let search = self.task_search.borrow().clone();
        let focus_mins = self.prefs.snapshot().default_session_mins;
        let choice = self.task_choice.borrow().clone();
        let (options, chosen) = self.handle.read(|s| {
            let chosen = choice.and_then(|id| {
                s.tasks.iter().find(|t| t.id == id).map(|t| (t.id.clone(), t.title.clone()))
            });
            (convert::task_options(s, today, &search, focus_mins), chosen)
        });
        let data = window.global::<AppData>();
        data.set_task_options(model(options));
        match chosen {
            Some((id, title)) => {
                data.set_session_task_id(id.into());
                data.set_session_task_title(title.into());
            }
            None => {
                *self.task_choice.borrow_mut() = None;
                data.set_session_task_id("".into());
                data.set_session_task_title("".into());
            }
        }
    }

    fn apply_pomodoro_settings(&self) {
        let prefs = self.prefs.snapshot();
        let mut settings = PomodoroSettings::from_preferences(&prefs);
        if self.mode.borrow().as_str() == "custom" {
            settings.focus_secs = u64::from(prefs.pomodoro_custom_mins) * 60;
        }
        self.pomodoro.borrow_mut().configure(settings);
    }

    /// Stops the timer on a phase chosen by the user, keeping the elapsed time
    /// of the phase it leaves.
    fn switch_phase(&self, window: &MainWindow, target: PomodoroPhase) {
        let (phase, elapsed) = {
            let timer = self.pomodoro.borrow();
            (timer.phase(), timer.elapsed_in_phase())
        };
        self.record_time(phase, elapsed, false);
        self.apply_pomodoro_settings();
        self.pomodoro.borrow_mut().select(target);
        self.refresh_all(window);
    }

    fn wire_pomodoro(self: &Rc<Self>, window: &MainWindow) {
        let data = window.global::<AppData>();
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_ambient_selected(move |id| {
                hub.prefs.set_pomodoro_ambient(id.as_str());
                if let Some(w) = weak.upgrade() {
                    hub.refresh_pomodoro(&w);
                }
            });
        }
        {
            let hub = self.clone();
            data.on_session_name_edited(move |name| {
                *hub.session_name.borrow_mut() = name.to_string();
            });
        }
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
                let (phase, elapsed) = {
                    let timer = hub.pomodoro.borrow();
                    (timer.phase(), timer.elapsed_in_phase())
                };
                hub.pomodoro.borrow_mut().next();
                hub.record_time(phase, elapsed, false);
                // Ending a break early can start the next Today task.
                if phase != PomodoroPhase::Focus && hub.prefs.snapshot().start_next_after_early_break {
                    if let Some(id) = hub.tasks.first_today_task() {
                        *hub.task_choice.borrow_mut() = Some(id);
                        hub.pomodoro.borrow_mut().start();
                    }
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_pomodoro_previous(move || {
                let (phase, elapsed) = {
                    let timer = hub.pomodoro.borrow();
                    (timer.phase(), timer.elapsed_in_phase())
                };
                hub.pomodoro.borrow_mut().previous();
                hub.record_time(phase, elapsed, false);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_pomodoro_reset(move || {
                // Reset discards the round: nothing is logged as a session
                // or counted toward the task. Skip is the way to keep it.
                hub.pomodoro.borrow_mut().reset();
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_pomodoro_select(move |id| {
                let Some(w) = weak.upgrade() else {
                    return;
                };
                let id = id.to_string();
                if id == "custom" || id == "focus" {
                    *hub.mode.borrow_mut() = id.clone();
                    hub.switch_phase(&w, PomodoroPhase::Focus);
                } else if let Some(target) = PomodoroPhase::from_str_id(&id) {
                    hub.switch_phase(&w, target);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_task_picked(move |id| {
                let id = id.to_string();
                *hub.task_choice.borrow_mut() = if id.is_empty() { None } else { Some(id) };
                if let Some(w) = weak.upgrade() {
                    hub.refresh_task_picker(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_task_search_changed(move |text| {
                *hub.task_search.borrow_mut() = text.to_string();
                if let Some(w) = weak.upgrade() {
                    hub.refresh_task_picker(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_activity_range_selected(move |range| {
                *hub.activity_range.borrow_mut() = range.to_string();
                if let Some(w) = weak.upgrade() {
                    hub.refresh_activity(&w);
                }
            });
        }
    }

    /// The first space, or "" when none has been created; tasks are then
    /// not added.
    fn default_list_id(&self) -> String {
        self.handle.read(|s| {
            s.lists.iter().find(|list| !list.is_archived).map(|list| list.id.clone()).unwrap_or_default()
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
                        "pomodoro_custom_mins" => current.pomodoro_custom_mins,
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
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_set_setting(move |key, value| {
                let _ = hub.prefs.set_int(&key, value);
                hub.apply_pomodoro_settings();
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_set_setting_flag(move |key, on| {
                let _ = hub.prefs.set_bool(&key, on);
                hub.apply_pomodoro_settings();
                // With breaks off, a break on show gives way to focus.
                let on_break = hub.pomodoro.borrow().phase() != PomodoroPhase::Focus;
                if key.as_str() == "pomodoro_breaks" && !on && on_break {
                    if let Some(w) = weak.upgrade() {
                        hub.switch_phase(&w, PomodoroPhase::Focus);
                    }
                }
                if let Some(w) = weak.upgrade() {
                    hub.refresh_all(&w);
                }
            });
        }
        {
            let weak = window.as_weak();
            data.on_language_search(move |query| {
                if let Some(w) = weak.upgrade() {
                    w.global::<AppData>().set_language_rows(model(language_rows(&query)));
                }
            });
        }
        {
            let hub = self.clone();
            let weak = window.as_weak();
            data.on_set_language(move |id| {
                let _ = hub.prefs.set_text("language", &id);
                let _ = slint::select_bundled_translation(&id);
                if let Some(w) = weak.upgrade() {
                    hub.refresh_settings(&w);
                }
            });
        }
    }
}

/// Languages whose name or id contains `query` (any case), in alphabetical
/// order by name.
fn language_rows(query: &str) -> Vec<LanguageRow> {
    let q = query.trim().to_lowercase();
    let mut rows: Vec<_> = LANGUAGE_CHOICES
        .iter()
        .filter(|(id, name, _)| q.is_empty() || name.to_lowercase().contains(&q) || *id == q)
        .collect();
    rows.sort_by_key(|(_, name, _)| name.to_lowercase());
    rows.into_iter()
        .map(|(id, name, _)| LanguageRow { id: (*id).into(), name: (*name).into(), header: "".into() })
        .collect()
}

/// Timer state shown to the user: Running, Paused, Completed or Ready.
fn pomodoro_status(timer: &PomodoroTimer) -> &'static str {
    if timer.is_running() {
        "Running"
    } else if timer.is_finished() {
        "Completed"
    } else if timer.remaining_secs() == timer.phase_total_secs() {
        "Ready"
    } else {
        "Paused"
    }
}

/// The tab to highlight: a focus round is "custom" only in custom mode.
fn display_mode(phase: PomodoroPhase, mode: &str) -> &'static str {
    match phase {
        PomodoroPhase::Focus if mode == "custom" => "custom",
        PomodoroPhase::Focus => "focus",
        PomodoroPhase::ShortBreak => "short_break",
        PomodoroPhase::LongBreak => "long_break",
    }
}

/// Percentage change from `before` to `now`, as text such as "+33%".
fn change(now: u64, before: u64) -> String {
    if before == 0 {
        return if now == 0 { String::new() } else { "New".to_string() };
    }
    let pct = (now as i64 - before as i64) * 100 / before as i64;
    if pct >= 0 { format!("+{pct}%") } else { format!("{pct}%") }
}
