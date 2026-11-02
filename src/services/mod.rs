//! Service layer: business workflows over the domain models.
//!
//! One file per area. Each service owns its behaviour and talks to storage
//! only through `StoreHandle`, which persists after every change.

pub mod alerts;
pub mod assistant;
pub mod history;
pub mod insights;
pub mod lists;
pub mod notes;
pub mod pomodoro;
pub mod preferences;
pub mod shield;
pub mod shortcuts;
pub mod store_handle;
pub mod tasks;
pub mod timer;
pub mod today;

pub use assistant::AssistantService;
pub use history::{ActionHistory, MoveSection, UndoableAction};
pub use insights::InsightsService;
pub use lists::ListService;
pub use notes::NotesService;
pub use pomodoro::{PomodoroPhase, PomodoroSettings, PomodoroTimer};
pub use preferences::PreferencesService;
pub use shield::ShieldService;
pub use store_handle::StoreHandle;
pub use tasks::TaskService;
pub use timer::{
    TickOutcome, TimerMode, TimerPhase, TimerPurpose, TimerService, TimerSnapshot, DEFAULT_BREAK_MINS,
};
pub use today::DayPlanService;
