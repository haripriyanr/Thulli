//! Task workflows: smart entry, editing, status changes, reordering, steps,
//! board and matrix views, calendar scheduling and global search.
//!
//! Every method that changes tasks on behalf of the UI takes the shared
//! [`ActionHistory`] and records one undoable [`TaskChange`], so the board,
//! matrix and calendar screens get undo and redo without extra wiring.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Local, NaiveDate, Utc};

use crate::models::{AppStateStore, MatrixQuadrant, Task, TaskSection, TaskStatus};
use crate::parser::parse_task_title;
use crate::repository::StorageRepository;
use crate::services::history::{ActionHistory, UndoableAction};
use crate::services::store_handle::StoreHandle;

// ---------------------------------------------------------------------------
// Undo support
// ---------------------------------------------------------------------------

/// A reversible change to a set of tasks, stored as full before and after
/// snapshots. Tasks missing from one side were created or removed by the change.
pub struct TaskChange {
    label: String,
    before: Vec<Task>,
    after: Vec<Task>,
}

impl TaskChange {
    fn restore(store: &mut AppStateStore, from: &[Task], target: &[Task]) {
        // Tasks the target state does not have must disappear; the rest are put back.
        let keep: HashSet<&str> = target.iter().map(|t| t.id.as_str()).collect();
        let gone: HashSet<&str> = from.iter().map(|t| t.id.as_str()).filter(|id| !keep.contains(id)).collect();
        store.tasks.retain(|t| !gone.contains(t.id.as_str()));
        for task in target {
            match store.tasks.iter_mut().find(|t| t.id == task.id) {
                Some(slot) => *slot = task.clone(),
                None => store.tasks.push(task.clone()),
            }
        }
    }
}

impl UndoableAction for TaskChange {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn undo(&self, store: &mut AppStateStore) {
        Self::restore(store, &self.after, &self.before);
    }

    fn redo(&self, store: &mut AppStateStore) {
        Self::restore(store, &self.before, &self.after);
    }
}

// ---------------------------------------------------------------------------
// Value types
// ---------------------------------------------------------------------------

/// Fields saved by the inline task editor.
#[derive(Debug, Clone, Default)]
pub struct TaskEdit {
    pub title: String,
    pub notes: String,
    pub estimate_mins: u32,
    pub tag_ids: Vec<String>,
    pub do_date: Option<NaiveDate>,
    pub deadline: Option<NaiveDate>,
}

/// Where a task moves within its column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDirection {
    Top,
    Up,
    Down,
    Bottom,
}

impl MoveDirection {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "top" => Some(Self::Top),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }
}

/// The board's Dates filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateFilter {
    #[default]
    Any,
    DueToday,
    DueThisWeek,
    Overdue,
    NoDate,
}

impl DateFilter {
    pub fn parse(value: &str) -> Self {
        match value {
            "today" => Self::DueToday,
            "week" => Self::DueThisWeek,
            "overdue" => Self::Overdue,
            "none" => Self::NoDate,
            _ => Self::Any,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::DueToday => "today",
            Self::DueThisWeek => "week",
            Self::Overdue => "overdue",
            Self::NoDate => "none",
        }
    }

    /// True when the task passes this filter on `today`.
    pub fn matches(&self, task: &Task, today: NaiveDate) -> bool {
        let week_end = today + Duration::days(6);
        let in_range = |d: Option<NaiveDate>, from: NaiveDate, to: NaiveDate| d.is_some_and(|d| d >= from && d <= to);
        match self {
            Self::Any => true,
            Self::DueToday => task.do_date == Some(today) || task.deadline == Some(today),
            Self::DueThisWeek => in_range(task.do_date, today, week_end) || in_range(task.deadline, today, week_end),
            Self::Overdue => task.is_pending() && task.deadline.is_some_and(|d| d < today),
            Self::NoDate => task.do_date.is_none() && task.deadline.is_none(),
        }
    }
}

/// Which tasks the board shows: the sidebar selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardScope {
    /// Every task from every list that is not archived.
    All,
    Logbook,
    Waiting,
    List(String),
}

impl BoardScope {
    /// Parses the sidebar filter: `all`, `logbook`, `waiting` or a list id.
    pub fn parse(value: &str) -> Self {
        match value {
            "" | "all" => Self::All,
            "logbook" => Self::Logbook,
            "waiting" => Self::Waiting,
            id => Self::List(id.to_string()),
        }
    }

    /// True for scopes shown as one flat list instead of four columns.
    pub fn is_flat(&self) -> bool {
        matches!(self, Self::Logbook | Self::Waiting)
    }
}

/// One board column with its progress figures.
#[derive(Debug, Clone)]
pub struct ColumnView {
    /// `backlog`, `this_week`, `today` or `done`.
    pub section: &'static str,
    pub title: &'static str,
    pub done: usize,
    pub total: usize,
    pub tasks: Vec<Task>,
}

/// Everything the board screen needs for one refresh.
#[derive(Debug, Clone, Default)]
pub struct BoardView {
    pub columns: Vec<ColumnView>,
    /// Used by the Logbook and Waiting scopes.
    pub flat: Vec<Task>,
}

/// Parses text typed into a date field. Empty clears the date; `today`,
/// `tomorrow` and `YYYY-MM-DD` are accepted.
pub fn parse_date_input(text: &str, today: NaiveDate) -> Result<Option<NaiveDate>, String> {
    let t = text.trim().to_lowercase();
    match t.as_str() {
        "" | "none" | "clear" => Ok(None),
        "today" => Ok(Some(today)),
        "tomorrow" => Ok(Some(today + Duration::days(1))),
        other => NaiveDate::parse_from_str(other, "%Y-%m-%d")
            .map(Some)
            .map_err(|_| format!("\"{}\" is not a date. Use YYYY-MM-DD, today or tomorrow.", text.trim())),
    }
}

fn top_level_sorted(mut tasks: Vec<&Task>) -> Vec<Task> {
    tasks.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.created_at.cmp(&b.created_at)));
    tasks.into_iter().cloned().collect()
}

/// Ids of a task and everything nested under it.
pub fn with_descendants(store: &AppStateStore, task_id: &str) -> Vec<String> {
    let mut ids = vec![task_id.to_string()];
    let mut i = 0;
    while i < ids.len() {
        let parent = ids[i].clone();
        ids.extend(store.tasks.iter().filter(|t| t.parent_id.as_deref() == Some(parent.as_str())).map(|t| t.id.clone()));
        i += 1;
    }
    ids
}

/// Nested breakdown steps of a task in display order, each with its depth
/// (1 for direct steps).
pub fn steps_flat(store: &AppStateStore, task_id: &str) -> Vec<(Task, i32)> {
    fn walk(store: &AppStateStore, parent: &str, depth: i32, out: &mut Vec<(Task, i32)>) {
        if depth > 16 {
            return;
        }
        for step in store.steps_of(parent) {
            out.push((step.clone(), depth));
            walk(store, &step.id, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(store, task_id, 1, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Service coordinating task creation, state changes and list associations.
pub struct TaskService {
    handle: StoreHandle,
}

impl TaskService {
    pub fn new(store: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Self {
        Self { handle: StoreHandle::new(store, repository) }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    /// Runs `f` and records one undoable change covering `ids` (and their
    /// nested steps) plus any task `f` created.
    fn tracked<R>(
        &self,
        history: &ActionHistory,
        label: &str,
        ids: &[String],
        f: impl FnOnce(&mut AppStateStore) -> R,
    ) -> R {
        let (result, change) = self.handle.mutate(|st| {
            let known: HashSet<String> = st.tasks.iter().map(|t| t.id.clone()).collect();
            let mut affected: Vec<String> = Vec::new();
            for id in ids {
                for d in with_descendants(st, id) {
                    if !affected.contains(&d) {
                        affected.push(d);
                    }
                }
            }
            let before: Vec<Task> = affected.iter().filter_map(|id| st.task(id).cloned()).collect();
            let result = f(st);
            let mut after_ids = affected;
            after_ids.extend(st.tasks.iter().filter(|t| !known.contains(&t.id)).map(|t| t.id.clone()));
            let after: Vec<Task> = after_ids.iter().filter_map(|id| st.task(id).cloned()).collect();
            (result, TaskChange { label: label.to_string(), before, after })
        });
        if !(change.before.is_empty() && change.after.is_empty()) {
            history.record(Box::new(change));
        }
        result
    }

    fn next_sort_order(st: &AppStateStore) -> i32 {
        st.tasks.iter().map(|t| t.sort_order).max().map_or(0, |m| m + 1)
    }

    /// Parses user input, resolves list fallback, and persists the new task.
    /// `default_section` is used when the text carries no section tag.
    pub fn add_smart_task(&self, raw_input: &str, list_id: &str, default_section: TaskSection) -> Option<Task> {
        let cleaned = raw_input.trim();
        if cleaned.is_empty() {
            return None;
        }

        self.handle.mutate(|st| {
            let parsed = if st.preferences.auto_parse_estimates {
                let p = parse_task_title(cleaned, default_section.as_str());
                (p.cleaned_title, p.estimate_mins, TaskSection::from_legacy_board(&p.target_board))
            } else {
                (cleaned.to_string(), 0, default_section)
            };

            // A task needs a space. With none created yet, nothing is added.
            let effective_list_id = if list_id.is_empty() || !st.lists.iter().any(|l| l.id == list_id) {
                st.get_primary_list()?.id.clone()
            } else {
                list_id.to_string()
            };

            let mut task = Task::new(effective_list_id, parsed.0, parsed.1, parsed.2);
            task.sort_order = Self::next_sort_order(st);
            Some(st.add_task(task))
        })
    }

    /// Like [`add_smart_task`](Self::add_smart_task), and undoable.
    pub fn add_task_recorded(
        &self,
        history: &ActionHistory,
        raw_input: &str,
        list_id: &str,
        default_section: TaskSection,
    ) -> Option<Task> {
        let task = self.add_smart_task(raw_input, list_id, default_section)?;
        history.record(Box::new(TaskChange { label: "Add task".to_string(), before: Vec::new(), after: vec![task.clone()] }));
        Some(task)
    }

    /// Toggles task completion and saves state.
    pub fn toggle_task(&self, task_id: &str) -> Option<bool> {
        self.handle.mutate(|st| st.toggle_task(task_id))
    }

    /// Undoable completion toggle. Returns true when the task is now completed.
    pub fn toggle_complete(&self, history: &ActionHistory, task_id: &str) -> Option<bool> {
        self.tracked(history, "Complete task", &[task_id.to_string()], |st| st.toggle_task(task_id))
    }

    /// Sets an explicit status (completed, cancelled, waiting, open).
    pub fn set_status(&self, task_id: &str, status: TaskStatus) -> bool {
        self.handle.mutate(|st| st.task_mut(task_id).map(|t| t.set_status(status)).is_some())
    }

    /// Undoable status change. Setting the status a task already has reopens it,
    /// so pressing a status key twice toggles it off.
    pub fn set_status_recorded(&self, history: &ActionHistory, task_id: &str, status: TaskStatus) -> bool {
        self.tracked(history, "Change status", &[task_id.to_string()], |st| {
            st.task_mut(task_id)
                .map(|t| {
                    let target = if t.status == status && status != TaskStatus::Open { TaskStatus::Open } else { status };
                    t.set_status(target)
                })
                .is_some()
        })
    }

    /// Moves a task to another planning section.
    pub fn move_to_section(&self, task_id: &str, section: TaskSection) -> bool {
        self.handle.mutate(|st| st.task_mut(task_id).map(|t| t.move_to_section(section)).is_some())
    }

    /// Undoable section move. A matrix quadrant that no longer fits the new
    /// section is cleared.
    pub fn move_to_section_recorded(&self, history: &ActionHistory, task_id: &str, section: TaskSection) -> bool {
        self.tracked(history, "Move task", &[task_id.to_string()], |st| {
            st.task_mut(task_id)
                .map(|t| {
                    t.move_to_section(section);
                    if t.matrix_quadrant.is_some_and(|q| q.section() != section) {
                        t.matrix_quadrant = None;
                    }
                })
                .is_some()
        })
    }

    /// Prepares a task for a focus run: it moves to Today and is reopened if
    /// it was waiting or cancelled. Returns the task's estimate in minutes.
    pub fn start_focus(&self, task_id: &str) -> Option<u32> {
        self.handle.mutate(|st| {
            st.task_mut(task_id).map(|t| {
                t.prepare_for_focus();
                t.estimate_mins
            })
        })
    }

    /// The first open task in Today, used by the global "start now" action.
    pub fn first_today_task(&self) -> Option<String> {
        self.handle.read(|st| {
            let candidates: Vec<&Task> = st
                .tasks
                .iter()
                .filter(|t| !t.is_step() && t.status == TaskStatus::Open && t.section == TaskSection::Today)
                .collect();
            top_level_sorted(candidates).first().map(|t| t.id.clone())
        })
    }

    /// Deletes a task and its breakdown steps.
    pub fn delete_task(&self, task_id: &str) -> bool {
        self.handle.mutate(|st| st.remove_task(task_id))
    }

    /// Undoable delete of a task and its steps.
    pub fn delete_recorded(&self, history: &ActionHistory, task_id: &str) -> bool {
        self.tracked(history, "Delete task", &[task_id.to_string()], |st| st.remove_task(task_id))
    }

    /// Undoable delete of several tasks as one step.
    pub fn delete_many(&self, history: &ActionHistory, ids: &[String]) -> usize {
        self.tracked(history, "Delete tasks", ids, |st| ids.iter().filter(|id| st.remove_task(id)).count())
    }

    /// Applies a status to several tasks as one undoable step.
    pub fn set_status_many(&self, history: &ActionHistory, ids: &[String], status: TaskStatus) -> usize {
        self.tracked(history, "Change status", ids, |st| {
            ids.iter().filter(|id| st.task_mut(id).map(|t| t.set_status(status)).is_some()).count()
        })
    }

    /// Moves several tasks to a section as one undoable step.
    pub fn move_many_to_section(&self, history: &ActionHistory, ids: &[String], section: TaskSection) -> usize {
        self.tracked(history, "Move tasks", ids, |st| {
            ids.iter()
                .filter(|id| {
                    st.task_mut(id)
                        .map(|t| {
                            t.move_to_section(section);
                            if t.matrix_quadrant.is_some_and(|q| q.section() != section) {
                                t.matrix_quadrant = None;
                            }
                        })
                        .is_some()
                })
                .count()
        })
    }

    /// Saves the inline editor. An empty title is rejected.
    pub fn update_task(&self, history: &ActionHistory, task_id: &str, edit: TaskEdit) -> bool {
        let title = edit.title.trim().to_string();
        if title.is_empty() {
            return false;
        }
        self.tracked(history, "Edit task", &[task_id.to_string()], |st| {
            let valid_tags: Vec<String> =
                edit.tag_ids.iter().filter(|id| st.tags.iter().any(|t| &t.id == *id)).cloned().collect();
            match st.task_mut(task_id) {
                Some(t) => {
                    t.title = title;
                    t.notes = edit.notes;
                    t.estimate_mins = edit.estimate_mins;
                    t.tags = valid_tags;
                    t.do_date = edit.do_date;
                    t.deadline = edit.deadline;
                    true
                }
                None => false,
            }
        })
    }

    /// Renames a task or step.
    pub fn rename(&self, history: &ActionHistory, task_id: &str, title: &str) -> bool {
        let title = title.trim().to_string();
        if title.is_empty() {
            return false;
        }
        self.tracked(history, "Rename task", &[task_id.to_string()], |st| {
            st.task_mut(task_id).map(|t| t.title = title).is_some()
        })
    }

    /// Replaces the note lines of a task.
    pub fn set_notes(&self, history: &ActionHistory, task_id: &str, notes: &str) -> bool {
        self.tracked(history, "Edit notes", &[task_id.to_string()], |st| {
            st.task_mut(task_id).map(|t| t.notes = notes.to_string()).is_some()
        })
    }

    /// Adds a breakdown step under `parent_id` (a task or another step).
    pub fn add_step(&self, history: &ActionHistory, parent_id: &str, title: &str) -> Option<Task> {
        let title = title.trim().to_string();
        if title.is_empty() {
            return None;
        }
        self.tracked(history, "Add step", &[parent_id.to_string()], |st| {
            let parent = st.task(parent_id)?.clone();
            let mut step = Task::new_step(&parent, title);
            step.sort_order = st.steps_of(parent_id).iter().map(|s| s.sort_order).max().map_or(0, |m| m + 1);
            Some(st.add_task(step))
        })
    }

    /// Moves a task within its column (the tasks that share its parent and section).
    pub fn reorder(&self, history: &ActionHistory, task_id: &str, direction: MoveDirection) -> bool {
        let siblings = self.handle.read(|st| {
            let t = st.task(task_id)?;
            let group: Vec<&Task> = st
                .tasks
                .iter()
                .filter(|o| o.parent_id == t.parent_id && (t.is_step() || o.section == t.section) && o.status == t.status)
                .collect();
            Some(top_level_sorted(group).into_iter().map(|o| o.id).collect::<Vec<_>>())
        });
        let Some(mut order) = siblings else { return false };
        let Some(pos) = order.iter().position(|id| id == task_id) else { return false };
        let new_pos = match direction {
            MoveDirection::Top => 0,
            MoveDirection::Up => pos.saturating_sub(1),
            MoveDirection::Down => (pos + 1).min(order.len() - 1),
            MoveDirection::Bottom => order.len() - 1,
        };
        if new_pos == pos {
            return false;
        }
        let id = order.remove(pos);
        order.insert(new_pos, id);
        self.tracked(history, "Reorder task", &order.clone(), |st| {
            for (i, id) in order.iter().enumerate() {
                if let Some(t) = st.task_mut(id) {
                    t.sort_order = i as i32;
                }
            }
            true
        })
    }

    /// Moves a task, and everything nested under it, to another list.
    pub fn assign_list(&self, history: &ActionHistory, task_id: &str, list_id: &str) -> bool {
        self.tracked(history, "Change list", &[task_id.to_string()], |st| {
            if !st.lists.iter().any(|l| l.id == list_id) {
                return false;
            }
            let ids = with_descendants(st, task_id);
            for id in ids {
                if let Some(t) = st.task_mut(&id) {
                    t.list_id = list_id.to_string();
                }
            }
            true
        })
    }

    /// Copies a task with its steps. The copy is open and sits at the end of the section.
    pub fn duplicate_task(&self, history: &ActionHistory, task_id: &str) -> Option<Task> {
        self.tracked(history, "Copy task", &[], |st| {
            let ids = with_descendants(st, task_id);
            let id_map: Vec<(String, String)> =
                ids.iter().map(|old| (old.clone(), uuid::Uuid::new_v4().to_string())).collect();
            let mut copies: Vec<Task> = Vec::new();
            for (old, new) in &id_map {
                let mut copy = st.task(old)?.clone();
                copy.id = new.clone();
                if let Some(parent) = copy.parent_id.clone() {
                    if let Some((_, new_parent)) = id_map.iter().find(|(o, _)| *o == parent) {
                        copy.parent_id = Some(new_parent.clone());
                    }
                }
                copy.status = TaskStatus::Open;
                copy.completed_at = None;
                copy.time_taken_secs = 0;
                copy.last_worked_at = None;
                copy.created_at = Utc::now();
                copy.history.clear();
                copy.record("Copied");
                copies.push(copy);
            }
            copies.first_mut()?.sort_order = Self::next_sort_order(st);
            let root = copies[0].clone();
            for copy in copies {
                st.add_task(copy);
            }
            Some(root)
        })
    }

    /// Places a task in a matrix quadrant (or clears it) and moves it to the
    /// quadrant's section.
    pub fn set_quadrant(&self, history: &ActionHistory, task_id: &str, quadrant: Option<MatrixQuadrant>) -> bool {
        self.tracked(history, "Sort task", &[task_id.to_string()], |st| {
            st.task_mut(task_id)
                .map(|t| {
                    t.matrix_quadrant = quadrant;
                    if let Some(q) = quadrant {
                        t.move_to_section(q.section());
                    }
                })
                .is_some()
        })
    }

    /// Schedules a task at a moment (or clears the schedule). The do date
    /// follows the schedule, and a task scheduled for today joins Today.
    pub fn schedule_task(&self, history: &ActionHistory, task_id: &str, at: Option<DateTime<Utc>>) -> bool {
        self.tracked(history, "Schedule task", &[task_id.to_string()], |st| {
            st.task_mut(task_id)
                .map(|t| {
                    t.scheduled_at = at;
                    match at {
                        Some(when) => {
                            let day = when.with_timezone(&Local).date_naive();
                            t.do_date = Some(day);
                            if day == Local::now().date_naive() {
                                t.move_to_section(TaskSection::Today);
                            }
                            t.record("Scheduled");
                        }
                        None => t.record("Unscheduled"),
                    }
                })
                .is_some()
        })
    }

    /// Plans a task for a whole day, or takes it off the calendar with `None`.
    /// Any time of day is cleared; a task planned for today joins Today.
    pub fn set_do_date(&self, history: &ActionHistory, task_id: &str, day: Option<NaiveDate>) -> bool {
        self.tracked(history, "Plan task", &[task_id.to_string()], |st| {
            st.task_mut(task_id)
                .map(|t| {
                    t.scheduled_at = None;
                    t.do_date = day;
                    match day {
                        Some(day) => {
                            if day == Local::now().date_naive() {
                                t.move_to_section(TaskSection::Today);
                            }
                            t.record("Planned");
                        }
                        None => t.record("Unscheduled"),
                    }
                })
                .is_some()
        })
    }

    /// Creates a task already scheduled at `at` (used by clicking a calendar slot).
    pub fn create_scheduled(
        &self,
        history: &ActionHistory,
        raw_input: &str,
        list_id: &str,
        at: DateTime<Utc>,
    ) -> Option<Task> {
        let created = self.add_smart_task(raw_input, list_id, TaskSection::ThisWeek)?;
        let id = created.id.clone();
        history.record(Box::new(TaskChange { label: "Add task".to_string(), before: Vec::new(), after: vec![created] }));
        self.schedule_task(history, &id, Some(at));
        self.handle.read(|st| st.task(&id).cloned())
    }

    // -- Reading ----------------------------------------------------------

    /// Builds the board for a sidebar selection and Dates filter.
    pub fn board(&self, scope: &BoardScope, dates: DateFilter) -> BoardView {
        let today = Local::now().date_naive();
        self.handle.read(|st| build_board(st, scope, dates, today))
    }

    /// Number of open top-level tasks across all non-archived lists.
    pub fn open_count(&self) -> usize {
        self.handle.read(|st| {
            let archived: HashSet<&str> = st.lists.iter().filter(|l| l.is_archived).map(|l| l.id.as_str()).collect();
            st.tasks
                .iter()
                .filter(|t| !t.is_step() && t.is_pending() && !archived.contains(t.list_id.as_str()))
                .count()
        })
    }
}

fn build_board(st: &AppStateStore, scope: &BoardScope, dates: DateFilter, today: NaiveDate) -> BoardView {
    let archived: HashSet<&str> = st.lists.iter().filter(|l| l.is_archived).map(|l| l.id.as_str()).collect();
    let in_scope = |t: &&Task| -> bool {
        if t.is_step() {
            return false;
        }
        match scope {
            BoardScope::List(id) => &t.list_id == id,
            _ => !archived.contains(t.list_id.as_str()),
        }
    };
    let scoped: Vec<&Task> = st.tasks.iter().filter(in_scope).filter(|t| dates.matches(t, today)).collect();

    match scope {
        BoardScope::Logbook => {
            let mut done: Vec<&Task> = scoped
                .iter()
                .copied()
                .filter(|t| matches!(t.status, TaskStatus::Completed | TaskStatus::Cancelled))
                .collect();
            done.sort_by(|a, b| b.completed_at.unwrap_or(b.created_at).cmp(&a.completed_at.unwrap_or(a.created_at)));
            BoardView { columns: Vec::new(), flat: done.into_iter().cloned().collect() }
        }
        BoardScope::Waiting => {
            let waiting: Vec<&Task> = scoped.iter().copied().filter(|t| t.status == TaskStatus::Waiting).collect();
            BoardView { columns: Vec::new(), flat: top_level_sorted(waiting) }
        }
        _ => {
            let mut columns = Vec::new();
            for section in [TaskSection::Backlog, TaskSection::ThisWeek, TaskSection::Today] {
                let pending: Vec<&Task> = scoped
                    .iter()
                    .copied()
                    .filter(|t| t.section == section && t.is_pending())
                    .collect();
                let finished = scoped.iter().filter(|t| t.section == section && t.is_completed()).count();
                columns.push(ColumnView {
                    section: section.as_str(),
                    title: section.title(),
                    done: finished,
                    total: pending.len() + finished,
                    tasks: top_level_sorted(pending),
                });
            }
            let mut completed: Vec<&Task> = scoped.iter().copied().filter(|t| t.is_completed()).collect();
            completed.sort_by(|a, b| b.completed_at.unwrap_or(b.created_at).cmp(&a.completed_at.unwrap_or(a.created_at)));
            let pending_all = scoped.iter().filter(|t| t.is_pending()).count();
            columns.push(ColumnView {
                section: "done",
                title: "Done",
                done: completed.len(),
                total: completed.len() + pending_all,
                tasks: completed.into_iter().cloned().collect(),
            });
            BoardView { columns, flat: Vec::new() }
        }
    }
}

// ---------------------------------------------------------------------------
// Matrix
// ---------------------------------------------------------------------------

/// Matrix quadrants and the review queue.
pub mod matrix {
    use super::*;

    /// Tasks per quadrant plus the unsorted queue, grouped by section.
    #[derive(Debug, Clone, Default)]
    pub struct MatrixView {
        pub do_asap: Vec<Task>,
        pub do_later_today: Vec<Task>,
        pub do_mid_week: Vec<Task>,
        pub leave_till_end: Vec<Task>,
        pub unsorted_today: Vec<Task>,
        pub unsorted_this_week: Vec<Task>,
        pub unsorted_backlog: Vec<Task>,
    }

    impl MatrixView {
        pub fn unsorted_count(&self) -> usize {
            self.unsorted_today.len() + self.unsorted_this_week.len() + self.unsorted_backlog.len()
        }
    }

    fn eligible<'a>(st: &'a AppStateStore, list_filter: &str) -> Vec<&'a Task> {
        let archived: HashSet<&str> = st.lists.iter().filter(|l| l.is_archived).map(|l| l.id.as_str()).collect();
        st.tasks
            .iter()
            .filter(|t| !t.is_step() && t.is_pending())
            .filter(|t| if list_filter.is_empty() { !archived.contains(t.list_id.as_str()) } else { t.list_id == list_filter })
            .collect()
    }

    /// Builds the matrix for an optional list filter (empty means every list).
    pub fn build(st: &AppStateStore, list_filter: &str) -> MatrixView {
        let tasks = eligible(st, list_filter);
        let pick = |q: Option<MatrixQuadrant>, section: Option<TaskSection>| -> Vec<Task> {
            top_level_sorted(
                tasks
                    .iter()
                    .copied()
                    .filter(|t| t.matrix_quadrant == q && section.is_none_or(|s| t.section == s))
                    .collect(),
            )
        };
        MatrixView {
            do_asap: pick(Some(MatrixQuadrant::DoAsap), None),
            do_later_today: pick(Some(MatrixQuadrant::DoLaterToday), None),
            do_mid_week: pick(Some(MatrixQuadrant::DoMidWeek), None),
            leave_till_end: pick(Some(MatrixQuadrant::LeaveTillEnd), None),
            unsorted_today: pick(None, Some(TaskSection::Today)),
            unsorted_this_week: pick(None, Some(TaskSection::ThisWeek)),
            unsorted_backlog: pick(None, Some(TaskSection::Backlog)),
        }
    }

    /// Ids to walk through in "prioritize one by one": unsorted tasks, most
    /// urgent section first.
    pub fn unsorted_queue(view: &MatrixView) -> Vec<String> {
        view.unsorted_today
            .iter()
            .chain(&view.unsorted_this_week)
            .chain(&view.unsorted_backlog)
            .map(|t| t.id.clone())
            .collect()
    }

    /// Ids of the tasks in one quadrant, for reviewing just that quadrant.
    pub fn quadrant_queue(view: &MatrixView, quadrant: MatrixQuadrant) -> Vec<String> {
        let tasks = match quadrant {
            MatrixQuadrant::DoAsap => &view.do_asap,
            MatrixQuadrant::DoLaterToday => &view.do_later_today,
            MatrixQuadrant::DoMidWeek => &view.do_mid_week,
            MatrixQuadrant::LeaveTillEnd => &view.leave_till_end,
        };
        tasks.iter().map(|t| t.id.clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// Calendar
// ---------------------------------------------------------------------------

/// Calendar ranges, scheduled items and the unscheduled rail.
pub mod calendar {
    use super::*;
    use chrono::{Datelike, Months, NaiveTime, TimeZone, Timelike, Weekday};

    use crate::models::{TimeEntryKind, WeekStart};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CalendarView {
        Day,
        Week,
        Month,
    }

    impl CalendarView {
        pub fn parse(value: &str) -> Self {
            match value {
                "day" => Self::Day,
                "month" => Self::Month,
                _ => Self::Week,
            }
        }

        pub fn as_str(&self) -> &'static str {
            match self {
                Self::Day => "day",
                Self::Week => "week",
                Self::Month => "month",
            }
        }

        /// Number of day cells shown: 1, 7 or 42 (six weeks).
        pub fn day_count(&self) -> i64 {
            match self {
                Self::Day => 1,
                Self::Week => 7,
                Self::Month => 42,
            }
        }
    }

    fn weekday_of(ws: WeekStart) -> Weekday {
        match ws {
            WeekStart::Monday => Weekday::Mon,
            WeekStart::Sunday => Weekday::Sun,
            WeekStart::Saturday => Weekday::Sat,
        }
    }

    /// First day of the week containing `date`.
    pub fn start_of_week(date: NaiveDate, ws: WeekStart) -> NaiveDate {
        let first = weekday_of(ws).num_days_from_monday() as i64;
        let current = date.weekday().num_days_from_monday() as i64;
        date - Duration::days((current - first).rem_euclid(7))
    }

    /// The first visible day for a view anchored at `anchor`.
    pub fn range_start(view: CalendarView, anchor: NaiveDate, ws: WeekStart) -> NaiveDate {
        match view {
            CalendarView::Day => anchor,
            CalendarView::Week => start_of_week(anchor, ws),
            CalendarView::Month => start_of_week(anchor.with_day(1).unwrap_or(anchor), ws),
        }
    }

    /// Moves the anchor one step back or forward (`delta` of -1 or 1).
    pub fn shift(view: CalendarView, anchor: NaiveDate, delta: i32) -> NaiveDate {
        match view {
            CalendarView::Day => anchor + Duration::days(delta as i64),
            CalendarView::Week => anchor + Duration::days(7 * delta as i64),
            CalendarView::Month => {
                let months = Months::new(delta.unsigned_abs());
                let moved = if delta >= 0 { anchor.checked_add_months(months) } else { anchor.checked_sub_months(months) };
                moved.unwrap_or(anchor)
            }
        }
    }

    /// Heading such as "3 Nov 2026", "2 to 8 Nov 2026" or "November 2026".
    pub fn range_label(view: CalendarView, anchor: NaiveDate, ws: WeekStart) -> String {
        match view {
            CalendarView::Day => anchor.format("%a %-d %b %Y").to_string(),
            CalendarView::Month => anchor.format("%B %Y").to_string(),
            CalendarView::Week => {
                let first = start_of_week(anchor, ws);
                let last = first + Duration::days(6);
                if first.month() == last.month() {
                    format!("{} to {}", first.format("%-d"), last.format("%-d %b %Y"))
                } else {
                    format!("{} to {}", first.format("%-d %b"), last.format("%-d %b %Y"))
                }
            }
        }
    }

    /// Local calendar slot to a UTC moment. Falls back to treating the time as UTC
    /// when the local time does not exist (daylight saving gaps).
    pub fn slot_to_utc(date: NaiveDate, start_mins: u32) -> DateTime<Utc> {
        let mins = start_mins.min(24 * 60 - 1);
        let time = NaiveTime::from_hms_opt(mins / 60, mins % 60, 0).unwrap_or_default();
        let naive = date.and_time(time);
        match Local.from_local_datetime(&naive).earliest() {
            Some(local) => local.with_timezone(&Utc),
            None => Utc.from_utc_datetime(&naive),
        }
    }

    /// Rounds minutes to the nearest `step` (at least 0, at most the last slot).
    pub fn snap_mins(mins: i32, step: i32) -> u32 {
        let step = step.max(1);
        let snapped = ((mins + step / 2) / step) * step;
        snapped.clamp(0, 24 * 60 - step) as u32
    }

    /// One scheduled task on the grid. `start_mins` is -1 for a task that has
    /// a do date but no time (shown as an all-day chip).
    #[derive(Debug, Clone, PartialEq)]
    pub struct CalendarItem {
        pub task_id: String,
        pub title: String,
        pub day_index: i32,
        pub start_mins: i32,
        pub length_mins: i32,
        pub color_hex: String,
    }

    /// A tracked block (planning, focus, break) for one day.
    #[derive(Debug, Clone, PartialEq)]
    pub struct TrackedBlock {
        pub id: String,
        pub kind: &'static str,
        pub label: String,
        pub start_mins: i32,
        pub length_mins: i32,
    }

    #[derive(Debug, Clone, Default)]
    pub struct CalendarData {
        pub items: Vec<CalendarItem>,
        pub unscheduled_today: Vec<Task>,
        pub unscheduled_this_week: Vec<Task>,
        pub unscheduled_backlog: Vec<Task>,
    }

    /// Items inside the visible range plus the unscheduled rail.
    pub fn build(
        st: &AppStateStore,
        view: CalendarView,
        anchor: NaiveDate,
        list_filter: &str,
    ) -> CalendarData {
        let first = range_start(view, anchor, st.preferences.week_start);
        let archived: HashSet<&str> = st.lists.iter().filter(|l| l.is_archived).map(|l| l.id.as_str()).collect();
        let tasks: Vec<&Task> = st
            .tasks
            .iter()
            .filter(|t| !t.is_step() && t.is_pending())
            .filter(|t| if list_filter.is_empty() { !archived.contains(t.list_id.as_str()) } else { t.list_id == list_filter })
            .collect();

        let mut data = CalendarData::default();
        for t in &tasks {
            let color = st.lists.iter().find(|l| l.id == t.list_id).map(|l| l.color.clone()).unwrap_or_default();
            let length = if t.estimate_mins == 0 { 30 } else { t.estimate_mins.max(15) as i32 };
            if let Some(at) = t.scheduled_at {
                let local = at.with_timezone(&Local);
                let idx = (local.date_naive() - first).num_days();
                if (0..view.day_count()).contains(&idx) {
                    let start = (local.hour() * 60 + local.minute()) as i32;
                    data.items.push(CalendarItem {
                        task_id: t.id.clone(),
                        title: t.title.clone(),
                        day_index: idx as i32,
                        start_mins: start,
                        length_mins: length,
                        color_hex: color,
                    });
                }
            } else if let Some(day) = t.do_date {
                let idx = (day - first).num_days();
                if (0..view.day_count()).contains(&idx) {
                    data.items.push(CalendarItem {
                        task_id: t.id.clone(),
                        title: t.title.clone(),
                        day_index: idx as i32,
                        start_mins: -1,
                        length_mins: length,
                        color_hex: color,
                    });
                }
            }
        }
        let unscheduled = |section: TaskSection| {
            top_level_sorted(
                tasks
                    .iter()
                    .copied()
                    .filter(|t| t.section == section && t.scheduled_at.is_none() && t.do_date.is_none())
                    .collect(),
            )
        };
        data.unscheduled_today = unscheduled(TaskSection::Today);
        data.unscheduled_this_week = unscheduled(TaskSection::ThisWeek);
        data.unscheduled_backlog = unscheduled(TaskSection::Backlog);
        data
    }

    /// Tracked time blocks that start on `date` (local time).
    pub fn tracked_blocks(st: &AppStateStore, date: NaiveDate) -> Vec<TrackedBlock> {
        st.time_entries
            .iter()
            .filter_map(|e| {
                let start = e.started_at.with_timezone(&Local);
                if start.date_naive() != date {
                    return None;
                }
                let start_mins = (start.hour() * 60 + start.minute()) as i32;
                let length = ((e.duration_secs() as i32 + 59) / 60).max(5);
                let label = if !e.label.is_empty() {
                    e.label.clone()
                } else if let Some(t) = e.task_id.as_deref().and_then(|id| st.task(id)) {
                    t.title.clone()
                } else {
                    match e.kind {
                        TimeEntryKind::Planning => "Planning".to_string(),
                        TimeEntryKind::Break => "Break".to_string(),
                        _ => "Focus".to_string(),
                    }
                };
                Some(TrackedBlock { id: e.id.clone(), kind: e.kind.as_str(), label, start_mins, length_mins: length })
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Global search
// ---------------------------------------------------------------------------

/// Global search: tasks, lists and quick actions.
pub mod search {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum HitKind {
        Task,
        List,
        Action,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SearchHit {
        pub kind: HitKind,
        /// Task id, list id or action name (`add-task`, `add-list`, `go-insights`).
        pub id: String,
        pub title: String,
        pub subtitle: String,
    }

    impl SearchHit {
        /// Id encoded for the UI: `task:<id>`, `list:<id>` or `action:<name>`.
        pub fn encoded_id(&self) -> String {
            let prefix = match self.kind {
                HitKind::Task => "task",
                HitKind::List => "list",
                HitKind::Action => "action",
            };
            format!("{prefix}:{}", self.id)
        }
    }

    const MAX_HITS: usize = 12;

    fn action(id: &str, title: &str) -> SearchHit {
        SearchHit { kind: HitKind::Action, id: id.to_string(), title: title.to_string(), subtitle: "Quick action".to_string() }
    }

    /// Finds tasks and lists whose name contains every word of `query`
    /// (case-insensitive) and offers quick actions.
    pub fn run(st: &AppStateStore, query: &str) -> Vec<SearchHit> {
        let q = query.trim().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        let matches = |text: &str| {
            let lower = text.to_lowercase();
            words.iter().all(|w| lower.contains(w))
        };

        let mut hits = Vec::new();
        if !words.is_empty() {
            for l in st.lists.iter().filter(|l| !l.is_archived && matches(&l.name)) {
                hits.push(SearchHit {
                    kind: HitKind::List,
                    id: l.id.clone(),
                    title: l.name.clone(),
                    subtitle: format!("List, {} to do", st.count_pending_for_list(&l.id)),
                });
            }
            for t in st.tasks.iter().filter(|t| matches(&t.title)) {
                let list = st.lists.iter().find(|l| l.id == t.list_id).map(|l| l.name.as_str()).unwrap_or("");
                let state = match t.status {
                    TaskStatus::Completed => "Done",
                    TaskStatus::Cancelled => "Cancelled",
                    TaskStatus::Waiting => "Waiting",
                    TaskStatus::Open => t.section.title(),
                };
                hits.push(SearchHit {
                    kind: HitKind::Task,
                    id: t.id.clone(),
                    title: t.title.clone(),
                    subtitle: format!("{list}, {state}"),
                });
            }
            hits.truncate(MAX_HITS);
        }

        let query_text = query.trim();
        if !query_text.is_empty() {
            hits.push(action("add-task-with-text", &format!("Add task: {query_text}")));
        }
        for (id, title) in [("add-task", "Add task"), ("add-list", "Add list"), ("go-insights", "Go to Insights")] {
            if words.is_empty() || matches(title) {
                hits.push(action(id, title));
            }
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Tag, TaskList, TimeEntry};
    use crate::repository::{LoadReport, StorageRepository};

    struct MemoryRepo;
    impl StorageRepository for MemoryRepo {
        fn load_with_report(&self) -> LoadReport {
            LoadReport { store: AppStateStore::default(), recovery: None }
        }
        fn save(&self, _: &AppStateStore) -> Result<(), Box<dyn std::error::Error>> {
            Ok(())
        }
    }

    fn setup() -> (TaskService, ActionHistory) {
        let svc = TaskService::new(Arc::new(Mutex::new(AppStateStore::with_space())), Arc::new(MemoryRepo));
        let history = ActionHistory::new(svc.handle().clone());
        (svc, history)
    }

    fn task(svc: &TaskService, title: &str, section: TaskSection) -> Task {
        svc.add_smart_task(title, "", section).unwrap()
    }

    fn get(svc: &TaskService, id: &str) -> Option<Task> {
        svc.handle().read(|s| s.task(id).cloned())
    }

    #[test]
    fn smart_entry_parses_estimate_and_section() {
        let (svc, _) = setup();
        let t = svc.add_smart_task("Write report 45m @week", "", TaskSection::Today).unwrap();
        assert_eq!(t.title, "Write report");
        assert_eq!(t.estimate_mins, 45);
        assert_eq!(t.section, TaskSection::ThisWeek);
    }

    #[test]
    fn parsing_can_be_switched_off() {
        let (svc, _) = setup();
        svc.handle().mutate(|st| st.preferences.auto_parse_estimates = false);
        let t = svc.add_smart_task("Write report 45m", "", TaskSection::Backlog).unwrap();
        assert_eq!(t.title, "Write report 45m");
        assert_eq!(t.section, TaskSection::Backlog);
    }

    #[test]
    fn unknown_list_falls_back_to_primary() {
        let (svc, _) = setup();
        let t = svc.add_smart_task("x", "missing", TaskSection::Today).unwrap();
        let primary = svc.handle().read(|s| s.get_primary_list().unwrap().id.clone());
        assert_eq!(t.list_id, primary);
    }

    #[test]
    fn starting_focus_moves_task_to_today() {
        let (svc, _) = setup();
        let t = task(&svc, "x", TaskSection::Backlog);
        svc.set_status(&t.id, TaskStatus::Waiting);
        assert_eq!(svc.start_focus(&t.id), Some(25));
        let t = get(&svc, &t.id).unwrap();
        assert_eq!((t.section, t.status), (TaskSection::Today, TaskStatus::Open));
        assert!(svc.start_focus("missing").is_none());
    }

    #[test]
    fn status_and_section_changes_apply() {
        let (svc, _) = setup();
        let t = task(&svc, "x", TaskSection::Today);
        assert!(svc.set_status(&t.id, TaskStatus::Waiting));
        assert!(svc.move_to_section(&t.id, TaskSection::Backlog));
        let st = get(&svc, &t.id).unwrap();
        assert_eq!((st.status, st.section), (TaskStatus::Waiting, TaskSection::Backlog));
    }

    #[test]
    fn status_key_pressed_twice_reopens() {
        let (svc, h) = setup();
        let t = task(&svc, "x", TaskSection::Today);
        svc.set_status_recorded(&h, &t.id, TaskStatus::Waiting);
        assert_eq!(get(&svc, &t.id).unwrap().status, TaskStatus::Waiting);
        svc.set_status_recorded(&h, &t.id, TaskStatus::Waiting);
        assert_eq!(get(&svc, &t.id).unwrap().status, TaskStatus::Open);
    }

    #[test]
    fn complete_undo_redo() {
        let (svc, h) = setup();
        let t = task(&svc, "x", TaskSection::Today);
        assert_eq!(svc.toggle_complete(&h, &t.id), Some(true));
        assert!(get(&svc, &t.id).unwrap().is_completed());
        h.undo();
        assert!(!get(&svc, &t.id).unwrap().is_completed());
        h.redo();
        assert!(get(&svc, &t.id).unwrap().is_completed());
    }

    #[test]
    fn recorded_add_and_delete_undo() {
        let (svc, h) = setup();
        let t = svc.add_task_recorded(&h, "Plan trip", "", TaskSection::Backlog).unwrap();
        assert!(get(&svc, &t.id).is_some());
        h.undo();
        assert!(get(&svc, &t.id).is_none());
        h.redo();
        assert!(get(&svc, &t.id).is_some());

        let step = svc.add_step(&h, &t.id, "Book hotel").unwrap();
        assert!(svc.delete_recorded(&h, &t.id));
        assert!(get(&svc, &t.id).is_none() && get(&svc, &step.id).is_none());
        h.undo();
        assert!(get(&svc, &t.id).is_some() && get(&svc, &step.id).is_some());
    }

    #[test]
    fn steps_nest_and_flatten_with_depth() {
        let (svc, h) = setup();
        let t = task(&svc, "Root", TaskSection::Today);
        let a = svc.add_step(&h, &t.id, "A").unwrap();
        let a1 = svc.add_step(&h, &a.id, "A1").unwrap();
        let b = svc.add_step(&h, &t.id, "B").unwrap();
        svc.add_step(&h, &a1.id, "A1x").unwrap();
        assert!(svc.add_step(&h, &t.id, "   ").is_none());
        let flat = svc.handle().read(|s| steps_flat(s, &t.id));
        let shape: Vec<(String, i32)> = flat.iter().map(|(t, d)| (t.title.clone(), *d)).collect();
        assert_eq!(shape, vec![("A".into(), 1), ("A1".into(), 2), ("A1x".into(), 3), ("B".into(), 1)]);
        assert_eq!(b.parent_id.as_deref(), Some(t.id.as_str()));
        assert_eq!(a.section, TaskSection::Today);
    }

    #[test]
    fn editor_saves_fields_and_rejects_empty_title() {
        let (svc, h) = setup();
        let tag = svc.handle().mutate(|st| {
            let tag = Tag::new("Deep", "#3CC9C4");
            st.tags.push(tag.clone());
            tag
        });
        let t = task(&svc, "Old", TaskSection::Today);
        let day = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let ok = svc.update_task(
            &h,
            &t.id,
            TaskEdit {
                title: " New ".into(),
                notes: "- note".into(),
                estimate_mins: 50,
                tag_ids: vec![tag.id.clone(), "ghost".into()],
                do_date: Some(day),
                deadline: None,
            },
        );
        assert!(ok);
        let saved = get(&svc, &t.id).unwrap();
        assert_eq!((saved.title.as_str(), saved.estimate_mins, saved.do_date), ("New", 50, Some(day)));
        assert_eq!(saved.tags, vec![tag.id]);
        assert!(!svc.update_task(&h, &t.id, TaskEdit { title: "  ".into(), ..Default::default() }));
        h.undo();
        assert_eq!(get(&svc, &t.id).unwrap().title, "Old");
    }

    #[test]
    fn reorder_moves_within_a_section() {
        let (svc, h) = setup();
        let a = task(&svc, "a", TaskSection::Today);
        let b = task(&svc, "b", TaskSection::Today);
        let c = task(&svc, "c", TaskSection::Today);
        let other = task(&svc, "o", TaskSection::Backlog);
        let order = |svc: &TaskService| -> Vec<String> {
            svc.board(&BoardScope::All, DateFilter::Any).columns[2].tasks.iter().map(|t| t.title.clone()).collect()
        };
        assert_eq!(order(&svc), ["a", "b", "c"]);
        assert!(svc.reorder(&h, &c.id, MoveDirection::Top));
        assert_eq!(order(&svc), ["c", "a", "b"]);
        assert!(svc.reorder(&h, &c.id, MoveDirection::Down));
        assert_eq!(order(&svc), ["a", "c", "b"]);
        assert!(svc.reorder(&h, &a.id, MoveDirection::Bottom));
        assert_eq!(order(&svc), ["c", "b", "a"]);
        assert!(!svc.reorder(&h, &a.id, MoveDirection::Down), "already last");
        assert!(svc.reorder(&h, &b.id, MoveDirection::Up));
        assert_eq!(order(&svc), ["b", "c", "a"]);
        h.undo();
        assert_eq!(order(&svc), ["c", "b", "a"]);
        assert_eq!(get(&svc, &other.id).unwrap().section, TaskSection::Backlog);
    }

    #[test]
    fn planning_a_day_clears_the_time_and_can_be_undone() {
        let (svc, h) = setup();
        let t = task(&svc, "x", TaskSection::Backlog);
        let today = Local::now().date_naive();
        svc.schedule_task(&h, &t.id, Some(calendar::slot_to_utc(today + Duration::days(2), 600)));
        svc.set_do_date(&h, &t.id, Some(today));
        let planned = get(&svc, &t.id).unwrap();
        assert_eq!((planned.do_date, planned.scheduled_at, planned.section), (Some(today), None, TaskSection::Today));
        svc.set_do_date(&h, &t.id, None);
        assert_eq!(get(&svc, &t.id).unwrap().do_date, None);
        h.undo();
        assert_eq!(get(&svc, &t.id).unwrap().do_date, Some(today));
    }

    #[test]
    fn moving_sections_clears_a_mismatched_quadrant() {
        let (svc, h) = setup();
        let t = task(&svc, "x", TaskSection::Today);
        svc.set_quadrant(&h, &t.id, Some(MatrixQuadrant::DoAsap));
        svc.move_to_section_recorded(&h, &t.id, TaskSection::Today);
        assert_eq!(get(&svc, &t.id).unwrap().matrix_quadrant, Some(MatrixQuadrant::DoAsap));
        svc.move_to_section_recorded(&h, &t.id, TaskSection::Backlog);
        assert_eq!(get(&svc, &t.id).unwrap().matrix_quadrant, None);
        h.undo();
        let back = get(&svc, &t.id).unwrap();
        assert_eq!((back.section, back.matrix_quadrant), (TaskSection::Today, Some(MatrixQuadrant::DoAsap)));
    }

    #[test]
    fn assign_list_moves_steps_too() {
        let (svc, h) = setup();
        let t = task(&svc, "x", TaskSection::Today);
        let step = svc.add_step(&h, &t.id, "s").unwrap();
        let list = svc.handle().mutate(|st| {
            let l = TaskList::new("Work", "#112233");
            st.add_list(l.clone());
            l
        });
        assert!(svc.assign_list(&h, &t.id, &list.id));
        assert_eq!(get(&svc, &step.id).unwrap().list_id, list.id);
        assert!(!svc.assign_list(&h, &t.id, "nope"));
    }

    #[test]
    fn duplicate_copies_steps_with_new_ids() {
        let (svc, h) = setup();
        let t = task(&svc, "Root", TaskSection::Today);
        let s = svc.add_step(&h, &t.id, "Step").unwrap();
        svc.add_step(&h, &s.id, "Sub").unwrap();
        svc.toggle_complete(&h, &t.id);
        let copy = svc.duplicate_task(&h, &t.id).unwrap();
        assert_ne!(copy.id, t.id);
        let (count, flat) = svc.handle().read(|st| (st.tasks.len(), steps_flat(st, &copy.id)));
        assert_eq!(count, 6);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[1].1, 2);
        assert_eq!(get(&svc, &copy.id).unwrap().status, TaskStatus::Open);
        h.undo();
        assert_eq!(svc.handle().read(|st| st.tasks.len()), 3);
    }

    #[test]
    fn bulk_actions_are_one_undo_step() {
        let (svc, h) = setup();
        let a = task(&svc, "a", TaskSection::Today);
        let b = task(&svc, "b", TaskSection::Today);
        let ids = vec![a.id.clone(), b.id.clone()];
        assert_eq!(svc.move_many_to_section(&h, &ids, TaskSection::Backlog), 2);
        assert_eq!(svc.set_status_many(&h, &ids, TaskStatus::Waiting), 2);
        assert_eq!(svc.delete_many(&h, &ids), 2);
        h.undo();
        assert_eq!(get(&svc, &a.id).unwrap().status, TaskStatus::Waiting);
        h.undo();
        assert_eq!(get(&svc, &a.id).unwrap().status, TaskStatus::Open);
        h.undo();
        assert_eq!(get(&svc, &b.id).unwrap().section, TaskSection::Today);
    }

    #[test]
    fn board_columns_report_progress() {
        let (svc, h) = setup();
        let a = task(&svc, "a", TaskSection::Today);
        task(&svc, "b", TaskSection::Today);
        task(&svc, "c", TaskSection::Backlog);
        svc.toggle_complete(&h, &a.id);
        let board = svc.board(&BoardScope::All, DateFilter::Any);
        let sections: Vec<&str> = board.columns.iter().map(|c| c.section).collect();
        assert_eq!(sections, ["backlog", "this_week", "today", "done"]);
        let today = &board.columns[2];
        assert_eq!((today.done, today.total, today.tasks.len()), (1, 2, 1));
        let done = &board.columns[3];
        assert_eq!((done.done, done.total, done.tasks.len()), (1, 3, 1));
    }

    #[test]
    fn logbook_and_waiting_scopes_are_flat() {
        let (svc, h) = setup();
        let a = task(&svc, "a", TaskSection::Today);
        let b = task(&svc, "b", TaskSection::Today);
        let c = task(&svc, "c", TaskSection::Today);
        svc.toggle_complete(&h, &a.id);
        svc.set_status_recorded(&h, &b.id, TaskStatus::Cancelled);
        svc.set_status_recorded(&h, &c.id, TaskStatus::Waiting);
        assert_eq!(svc.board(&BoardScope::Logbook, DateFilter::Any).flat.len(), 2);
        let waiting = svc.board(&BoardScope::Waiting, DateFilter::Any).flat;
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].id, c.id);
        assert!(BoardScope::parse("waiting").is_flat() && !BoardScope::parse("abc").is_flat());
    }

    #[test]
    fn list_scope_and_archived_lists() {
        let (svc, _) = setup();
        let other = svc.handle().mutate(|st| {
            let l = TaskList::new("Other", "#fff000");
            st.add_list(l.clone());
            l
        });
        let in_other = svc.add_smart_task("o", &other.id, TaskSection::Today).unwrap();
        task(&svc, "p", TaskSection::Today);
        let only = svc.board(&BoardScope::List(other.id.clone()), DateFilter::Any);
        assert_eq!(only.columns[2].tasks.len(), 1);
        svc.handle().mutate(|st| st.lists.iter_mut().find(|l| l.id == other.id).unwrap().toggle_archived());
        let all = svc.board(&BoardScope::All, DateFilter::Any);
        assert_eq!(all.columns[2].tasks.len(), 1);
        assert_ne!(all.columns[2].tasks[0].id, in_other.id);
        assert_eq!(svc.open_count(), 1);
    }

    #[test]
    fn dates_filter_matches() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let mut t = Task::new("l", "x", 0, TaskSection::Today);
        assert!(DateFilter::NoDate.matches(&t, today));
        assert!(!DateFilter::Overdue.matches(&t, today));
        t.deadline = Some(today - Duration::days(1));
        assert!(DateFilter::Overdue.matches(&t, today));
        t.deadline = Some(today + Duration::days(3));
        assert!(DateFilter::DueThisWeek.matches(&t, today));
        assert!(!DateFilter::DueToday.matches(&t, today));
        t.do_date = Some(today);
        assert!(DateFilter::DueToday.matches(&t, today));
        assert!(!DateFilter::NoDate.matches(&t, today));
        assert_eq!(DateFilter::parse("overdue"), DateFilter::Overdue);
        assert_eq!(DateFilter::parse("zzz"), DateFilter::Any);
    }

    #[test]
    fn date_input_parsing() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        assert_eq!(parse_date_input("", today), Ok(None));
        assert_eq!(parse_date_input("Today", today), Ok(Some(today)));
        assert_eq!(parse_date_input("tomorrow", today), Ok(Some(today + Duration::days(1))));
        assert_eq!(parse_date_input("2026-12-24", today), Ok(NaiveDate::from_ymd_opt(2026, 12, 24)));
        assert!(parse_date_input("next friday", today).is_err());
    }

    #[test]
    fn matrix_places_tasks_and_builds_queues() {
        let (svc, h) = setup();
        let a = task(&svc, "a", TaskSection::Backlog);
        let b = task(&svc, "b", TaskSection::Today);
        task(&svc, "c", TaskSection::ThisWeek);
        svc.set_quadrant(&h, &a.id, Some(MatrixQuadrant::DoMidWeek));
        let view = svc.handle().read(|st| matrix::build(st, ""));
        assert_eq!(view.do_mid_week.len(), 1);
        assert_eq!(get(&svc, &a.id).unwrap().section, TaskSection::ThisWeek);
        assert_eq!(view.unsorted_count(), 2);
        let queue = matrix::unsorted_queue(&view);
        assert_eq!(queue[0], b.id, "Today first");
        assert_eq!(matrix::quadrant_queue(&view, MatrixQuadrant::DoMidWeek), vec![a.id.clone()]);
        svc.set_quadrant(&h, &a.id, None);
        assert_eq!(svc.handle().read(|st| matrix::build(st, "").unsorted_count()), 3);
    }

    #[test]
    fn matrix_respects_list_filter() {
        let (svc, _) = setup();
        let other = svc.handle().mutate(|st| {
            let l = TaskList::new("Other", "#fff000");
            st.add_list(l.clone());
            l
        });
        svc.add_smart_task("o", &other.id, TaskSection::Today).unwrap();
        task(&svc, "p", TaskSection::Today);
        assert_eq!(svc.handle().read(|st| matrix::build(st, "").unsorted_count()), 2);
        assert_eq!(svc.handle().read(|st| matrix::build(st, &other.id).unsorted_count()), 1);
    }

    #[test]
    fn calendar_ranges_follow_week_start() {
        use calendar::*;
        use crate::models::WeekStart;
        let wed = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        assert_eq!(range_start(CalendarView::Week, wed, WeekStart::Monday), NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
        assert_eq!(range_start(CalendarView::Week, wed, WeekStart::Sunday), NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        assert_eq!(range_start(CalendarView::Week, wed, WeekStart::Saturday), NaiveDate::from_ymd_opt(2026, 10, 31).unwrap());
        assert_eq!(range_start(CalendarView::Month, wed, WeekStart::Monday), NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
        assert_eq!(range_start(CalendarView::Day, wed, WeekStart::Monday), wed);
        assert_eq!(shift(CalendarView::Week, wed, 1), NaiveDate::from_ymd_opt(2026, 11, 11).unwrap());
        assert_eq!(shift(CalendarView::Month, NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(), 1), NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
        assert_eq!(shift(CalendarView::Day, wed, -1), NaiveDate::from_ymd_opt(2026, 11, 3).unwrap());
        assert_eq!(range_label(CalendarView::Month, wed, WeekStart::Monday), "November 2026");
        assert_eq!(range_label(CalendarView::Week, wed, WeekStart::Monday), "2 to 8 Nov 2026");
        assert_eq!(snap_mins(37, 30), 30);
        assert_eq!(snap_mins(50, 30), 60);
        assert_eq!(snap_mins(5000, 30), 23 * 60 + 30);
        assert_eq!(snap_mins(-20, 30), 0);
        assert_eq!(CalendarView::parse("month").day_count(), 42);
    }

    #[test]
    fn scheduling_places_items_and_leaves_the_rail() {
        use calendar::*;
        let (svc, h) = setup();
        let t = task(&svc, "Write", TaskSection::Backlog);
        let other = task(&svc, "Idle", TaskSection::Today);
        let day = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        let at = slot_to_utc(day, 14 * 60 + 30);
        assert!(svc.schedule_task(&h, &t.id, Some(at)));
        let data = svc.handle().read(|st| build(st, CalendarView::Week, day, ""));
        assert_eq!(data.items.len(), 1);
        let item = &data.items[0];
        assert_eq!((item.day_index, item.start_mins, item.length_mins), (2, 14 * 60 + 30, 25));
        assert_eq!(data.unscheduled_today.len(), 1);
        assert_eq!(data.unscheduled_today[0].id, other.id);
        assert!(data.unscheduled_backlog.is_empty());
        // Outside the visible week it is not listed.
        let later = svc.handle().read(|st| build(st, CalendarView::Week, day + Duration::days(14), ""));
        assert!(later.items.is_empty());
        // Month view includes it too.
        let month = svc.handle().read(|st| build(st, CalendarView::Month, day, ""));
        assert_eq!(month.items.len(), 1);
        h.undo();
        assert!(svc.handle().read(|st| build(st, CalendarView::Week, day, "")).items.is_empty());
        assert!(svc.schedule_task(&h, &t.id, Some(at)));
        assert!(svc.schedule_task(&h, &t.id, None));
        assert!(get(&svc, &t.id).unwrap().scheduled_at.is_none());
    }

    #[test]
    fn do_date_only_tasks_show_as_all_day() {
        use calendar::*;
        let (svc, h) = setup();
        let t = task(&svc, "Sometime", TaskSection::Today);
        let day = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        svc.update_task(&h, &t.id, TaskEdit { title: "Sometime".into(), do_date: Some(day), ..Default::default() });
        let data = svc.handle().read(|st| build(st, CalendarView::Week, day, ""));
        assert_eq!(data.items.len(), 1);
        assert_eq!(data.items[0].start_mins, -1);
        assert!(data.unscheduled_today.is_empty());
    }

    #[test]
    fn creating_at_a_slot_schedules_the_task() {
        use calendar::*;
        let (svc, h) = setup();
        let day = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        let t = svc.create_scheduled(&h, "Call mum 30m", "", slot_to_utc(day, 9 * 60)).unwrap();
        assert!(t.scheduled_at.is_some());
        assert_eq!(t.title, "Call mum");
        assert_eq!(get(&svc, &t.id).unwrap().do_date, Some(day));
        h.undo();
        h.undo();
        assert!(get(&svc, &t.id).is_none());
    }

    #[test]
    fn tracked_blocks_come_from_time_entries() {
        use calendar::*;
        let (svc, _) = setup();
        let day = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        let start = slot_to_utc(day, 10 * 60);
        svc.handle().mutate(|st| {
            let mut e = TimeEntry::new(crate::models::TimeEntryKind::Planning, start, start + Duration::minutes(15));
            e.label.clear();
            st.time_entries.push(e);
        });
        let blocks = svc.handle().read(|st| tracked_blocks(st, day));
        assert_eq!(blocks.len(), 1);
        assert_eq!((blocks[0].kind, blocks[0].start_mins, blocks[0].length_mins), ("planning", 600, 15));
        assert_eq!(blocks[0].label, "Planning");
        assert!(svc.handle().read(|st| tracked_blocks(st, day + Duration::days(1))).is_empty());
    }

    #[test]
    fn search_finds_tasks_lists_and_actions() {
        let (svc, _) = setup();
        task(&svc, "Plan holiday", TaskSection::Today);
        task(&svc, "Pay rent", TaskSection::Backlog);
        let hits = svc.handle().read(|st| search::run(st, "HOLIDAY plan"));
        assert_eq!(hits[0].kind, search::HitKind::Task);
        assert!(hits[0].encoded_id().starts_with("task:"));
        assert!(hits.iter().any(|h| h.id == "add-task-with-text" && h.title == "Add task: HOLIDAY plan"));

        let personal = svc.handle().read(|st| search::run(st, "personal"));
        assert_eq!(personal[0].kind, search::HitKind::List);

        let empty = svc.handle().read(|st| search::run(st, ""));
        let ids: Vec<&str> = empty.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["add-task", "add-list", "go-insights"]);

        let insight = svc.handle().read(|st| search::run(st, "insights"));
        assert!(insight.iter().any(|h| h.id == "go-insights"));
        assert!(!insight.iter().any(|h| h.id == "add-list"));
    }

    #[test]
    fn first_today_task_skips_non_open() {
        let (svc, h) = setup();
        let a = task(&svc, "a", TaskSection::Today);
        let b = task(&svc, "b", TaskSection::Today);
        task(&svc, "c", TaskSection::Backlog);
        svc.toggle_complete(&h, &a.id);
        assert_eq!(svc.first_today_task(), Some(b.id.clone()));
        svc.set_status(&b.id, TaskStatus::Waiting);
        assert_eq!(svc.first_today_task(), None);
    }
}
