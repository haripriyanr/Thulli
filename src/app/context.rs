//! Shared state handed to every screen's `bind` function.

// The services and `on_tick` are the seam for the screen bindings that are still
// stubs; remove this once every field is used.
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use thulli_core::models::AppStateStore;
use thulli_core::repository::{LoadReport, StorageRepository};
use thulli_core::services::{
    ActionHistory, AssistantService, DayPlanService, InsightsService, ListService, NotesService, PreferencesService,
    ShieldService, StoreHandle, TaskService, TickOutcome, TimerService, TimerSnapshot,
};
use thulli_core::services::timer::FocusRunService;

type TickListener = Box<dyn Fn(&TickOutcome, &TimerSnapshot)>;

/// Everything a screen binding needs: the state, one service per area, the
/// focus timer, and the UI-thread timer that drives it.
///
/// Cheap to clone. It lives on the UI thread only (it holds `Rc`s).
#[derive(Clone)]
pub struct AppContext {
    pub store: Arc<Mutex<AppStateStore>>,
    pub repository: Arc<dyn StorageRepository>,
    pub handle: StoreHandle,

    pub tasks: Arc<TaskService>,
    pub lists: Arc<ListService>,
    pub today: Arc<DayPlanService>,
    pub notes: Arc<NotesService>,
    pub insights: Arc<InsightsService>,
    pub preferences: Arc<PreferencesService>,
    pub shield: Arc<ShieldService>,
    pub assistant: Arc<AssistantService>,
    /// Undo and redo for board actions.
    pub history: Arc<ActionHistory>,

    /// The single focus timer shared by every screen and the timer pill.
    pub timer: Arc<TimerService>,
    /// Focus runs: sessions, tasks, breaks and time entries, over `timer`.
    pub focus: Arc<FocusRunService>,
    /// Message to show in the banner on launch (set after data recovery).
    pub startup_notice: Option<String>,

    /// Drives `timer.tick()`; started by `shell::bind`.
    pub(crate) tick_timer: Rc<slint::Timer>,
    tick_listeners: Rc<RefCell<Vec<TickListener>>>,
}

impl AppContext {
    pub fn new(repository: Arc<dyn StorageRepository>, report: LoadReport) -> Self {
        let startup_notice = report.recovery.as_ref().map(|r| r.user_message());
        let default_session = report.store.preferences.default_session_mins;
        let store = Arc::new(Mutex::new(report.store));
        let handle = StoreHandle::new(store.clone(), repository.clone());
        let timer = Arc::new(TimerService::new(default_session));

        Self {
            tasks: Arc::new(TaskService::new(store.clone(), repository.clone())),
            lists: Arc::new(ListService::new(store.clone(), repository.clone())),
            today: Arc::new(DayPlanService::new(handle.clone())),
            notes: Arc::new(NotesService::new(handle.clone())),
            insights: Arc::new(InsightsService::new(handle.clone())),
            preferences: Arc::new(PreferencesService::new(handle.clone())),
            shield: Arc::new(ShieldService::new(handle.clone())),
            assistant: Arc::new(AssistantService::new(handle.clone())),
            history: Arc::new(ActionHistory::new(handle.clone())),
            focus: Arc::new(FocusRunService::new(handle.clone(), timer.clone())),
            timer,
            startup_notice,
            tick_timer: Rc::new(slint::Timer::default()),
            tick_listeners: Rc::new(RefCell::new(Vec::new())),
            store,
            repository,
            handle,
        }
    }

    /// Registers a callback run on every timer tick that advanced time or
    /// finished a countdown. Use it to accrue time to tasks and to react when
    /// a session ends.
    pub fn on_tick(&self, listener: impl Fn(&TickOutcome, &TimerSnapshot) + 'static) {
        self.tick_listeners.borrow_mut().push(Box::new(listener));
    }

    pub(crate) fn notify_tick(&self, outcome: &TickOutcome, snapshot: &TimerSnapshot) {
        for listener in self.tick_listeners.borrow().iter() {
            listener(outcome, snapshot);
        }
    }
}
