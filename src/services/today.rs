//! Daily workflow: plan, work and wrap up, plus the daily goal and the day
//! timeline.
//!
//! The service reads and writes state through the shared [`StoreHandle`].
//! Tracked time itself (planning, focus and break blocks) is recorded by the
//! focus run service in `timer.rs`; this service only reads it back.

use std::collections::HashSet;

use chrono::{DateTime, Local, NaiveDate, Timelike, Utc};

use crate::models::{AppStateStore, Task, TaskSection, TaskStatus, TimeEntryKind};
use crate::services::store_handle::StoreHandle;

/// Where the day stands in the Plan, Work, Wrap up flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayPhase {
    /// Nothing decided yet: offer to plan or skip planning.
    Empty,
    /// The planning screen is open and planning time is being tracked.
    Planning,
    /// Planning is done (or skipped) and the day is being worked.
    Working,
    /// The day was wrapped up.
    WrappedUp,
}

impl DayPhase {
    /// Identifier used by the UI layer.
    pub fn as_str(&self) -> &'static str {
        match self {
            DayPhase::Empty => "empty",
            DayPhase::Planning => "planning",
            DayPhase::Working => "working",
            DayPhase::WrappedUp => "wrapped",
        }
    }
}

/// Progress towards the daily goal, split by what the time was spent on.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalSummary {
    pub goal_secs: u64,
    pub worked_secs: u64,
    pub left_secs: u64,
    pub focus_secs: u64,
    pub meeting_secs: u64,
    pub other_secs: u64,
    /// 0 to 1.
    pub progress: f32,
}

/// One tracked block on the day timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineBlock {
    pub id: String,
    pub kind: TimeEntryKind,
    pub label: String,
    /// Minutes since local midnight.
    pub start_mins: i32,
    pub length_mins: i32,
}

/// A running block whose end has moved since it was last saved:
/// the entry id and its current end.
pub type LiveBlock = (String, DateTime<Utc>);

/// Local calendar date of a UTC timestamp.
pub fn local_date(at: DateTime<Utc>) -> NaiveDate {
    at.with_timezone(&Local).date_naive()
}

/// Ids of today's tasks still to do, in priority order: top-level, in the
/// Today section and open (waiting tasks are not actionable).
pub fn queue_ids(store: &AppStateStore) -> Vec<String> {
    let mut tasks: Vec<&Task> = store
        .tasks
        .iter()
        .filter(|t| !t.is_step() && t.section == TaskSection::Today && t.status == TaskStatus::Open)
        .collect();
    tasks.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.created_at.cmp(&b.created_at)));
    tasks.into_iter().map(|t| t.id.clone()).collect()
}

/// Formats a number of seconds as "2h 05m", "45m" or "0m".
pub fn format_duration(secs: u64) -> String {
    let mins = secs / 60;
    if mins >= 60 {
        format!("{}h {:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

/// Daily workflow: plan, work and wrap up, plus the day timeline.
pub struct DayPlanService {
    handle: StoreHandle,
}

impl DayPlanService {
    pub fn new(handle: StoreHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    /// Today's local date.
    pub fn today(&self) -> NaiveDate {
        Local::now().date_naive()
    }

    // ---- plan ----

    /// The phase of `date`. `planning_active` is true while planning time is
    /// being tracked.
    pub fn phase(&self, date: NaiveDate, planning_active: bool) -> DayPhase {
        let (planned, wrapped) = self.handle.read(|s| {
            s.day_plan(date).map(|p| (p.planned, p.wrapped_up)).unwrap_or((false, false))
        });
        if wrapped {
            DayPhase::WrappedUp
        } else if planning_active {
            DayPhase::Planning
        } else if planned {
            DayPhase::Working
        } else {
            DayPhase::Empty
        }
    }

    /// Skips planning: the day goes straight to working.
    pub fn skip_planning(&self, date: NaiveDate) {
        self.handle.mutate(|s| {
            s.day_plan_mut(date).planned = true;
        });
    }

    /// Marks planning finished and adds the time spent to the day.
    pub fn finish_planning(&self, date: NaiveDate, planning_secs: u64) {
        self.handle.mutate(|s| {
            let ids = queue_ids(s);
            let plan = s.day_plan_mut(date);
            plan.finish_planning(planning_secs);
            plan.planned_task_ids = ids;
        });
    }

    /// Returns a wrapped-up day to working.
    pub fn reopen_day(&self, date: NaiveDate) {
        self.handle.mutate(|s| {
            let plan = s.day_plan_mut(date);
            plan.wrapped_up = false;
            plan.wrapped_up_at = None;
        });
    }

    /// Ends the working day with an optional brain dump.
    pub fn wrap_up(&self, date: NaiveDate, brain_dump: &str) {
        self.handle.mutate(|s| {
            let plan = s.day_plan_mut(date);
            plan.planned = true;
            plan.wrap_up(brain_dump.trim());
        });
    }

    // ---- today's list ----

    /// Open tasks in Today, in priority order.
    pub fn priority_tasks(&self) -> Vec<Task> {
        self.handle.read(|s| {
            queue_ids(s).iter().filter_map(|id| s.task(id).cloned()).collect()
        })
    }

    /// Every task that can still be worked on, for the planning sidebar and
    /// the task picker: Today first, then This Week, then Backlog.
    pub fn candidate_tasks(&self) -> Vec<Task> {
        self.handle.read(|s| {
            let mut tasks: Vec<Task> = s
                .tasks
                .iter()
                .filter(|t| !t.is_step() && t.status == TaskStatus::Open)
                .cloned()
                .collect();
            let rank = |t: &Task| match t.section {
                TaskSection::Today => 0,
                TaskSection::ThisWeek => 1,
                TaskSection::Backlog => 2,
            };
            tasks.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.sort_order.cmp(&b.sort_order)));
            tasks
        })
    }

    /// Moves a task into Today (added at the end of the list).
    pub fn add_to_today(&self, task_id: &str) -> bool {
        self.handle.mutate(|s| {
            let next = s.tasks.iter().map(|t| t.sort_order).max().map_or(0, |m| m + 1);
            s.task_mut(task_id)
                .map(|t| {
                    t.move_to_section(TaskSection::Today);
                    t.sort_order = next;
                })
                .is_some()
        })
    }

    /// Takes a task out of Today and puts it in This Week.
    pub fn remove_from_today(&self, task_id: &str) -> bool {
        self.handle.mutate(|s| s.task_mut(task_id).map(|t| t.move_to_section(TaskSection::ThisWeek)).is_some())
    }

    // ---- daily goal ----

    pub fn set_daily_goal_mins(&self, mins: u32) {
        self.handle.mutate(|s| s.preferences.daily_goal_mins = mins.clamp(15, 24 * 60));
    }

    pub fn daily_goal_mins(&self) -> u32 {
        self.handle.read(|s| s.preferences.daily_goal_mins)
    }

    /// Worked time against the goal for `date`. Breaks do not count.
    pub fn goal_summary(&self, date: NaiveDate, live: Option<&LiveBlock>) -> GoalSummary {
        self.handle.read(|s| {
            let mut focus = 0;
            let mut meeting = 0;
            let mut other = 0;
            for (entry, end) in day_entries(s, date, live) {
                let secs = (end - entry.started_at).num_seconds().max(0) as u64;
                match entry.kind {
                    TimeEntryKind::Focus => focus += secs,
                    TimeEntryKind::Break => {}
                    TimeEntryKind::Planning => other += secs,
                    TimeEntryKind::Other => {
                        if entry.label.to_lowercase().contains("meeting") {
                            meeting += secs;
                        } else {
                            other += secs;
                        }
                    }
                }
            }
            let goal = u64::from(s.preferences.daily_goal_mins) * 60;
            let worked = focus + meeting + other;
            GoalSummary {
                goal_secs: goal,
                worked_secs: worked,
                left_secs: goal.saturating_sub(worked),
                focus_secs: focus,
                meeting_secs: meeting,
                other_secs: other,
                progress: if goal == 0 { 0.0 } else { (worked as f32 / goal as f32).min(1.0) },
            }
        })
    }

    // ---- day timeline ----

    /// Tracked blocks for `date`, earliest first.
    pub fn timeline(&self, date: NaiveDate, live: Option<&LiveBlock>) -> Vec<TimelineBlock> {
        self.handle.read(|s| {
            let mut blocks: Vec<TimelineBlock> = day_entries(s, date, live)
                .into_iter()
                .map(|(entry, end)| {
                    let start_local = entry.started_at.with_timezone(&Local);
                    let start_mins = (start_local.hour() * 60 + start_local.minute()) as i32;
                    let secs = (end - entry.started_at).num_seconds().max(0);
                    let length = (((secs + 59) / 60) as i32).max(1).min(24 * 60 - start_mins);
                    TimelineBlock {
                        id: entry.id.clone(),
                        kind: entry.kind,
                        label: entry.label.clone(),
                        start_mins,
                        length_mins: length.max(1),
                    }
                })
                .collect();
            blocks.sort_by_key(|b| b.start_mins);
            blocks
        })
    }

    /// Ids of tasks completed on `date`, among today's list.
    pub fn done_today_ids(&self, date: NaiveDate) -> HashSet<String> {
        self.handle.read(|s| done_ids(s, date))
    }
}

fn done_ids(s: &AppStateStore, date: NaiveDate) -> HashSet<String> {
    s.tasks
        .iter()
        .filter(|t| !t.is_step() && t.status == TaskStatus::Completed && t.completed_at.is_some_and(|c| local_date(c) == date))
        .map(|t| t.id.clone())
        .collect()
}

/// Entries that start on `date`, each with its effective end.
fn day_entries<'a>(
    s: &'a AppStateStore,
    date: NaiveDate,
    live: Option<&LiveBlock>,
) -> Vec<(&'a crate::models::TimeEntry, DateTime<Utc>)> {
    s.time_entries
        .iter()
        .filter(|e| local_date(e.started_at) == date)
        .map(|e| {
            let end = match live {
                Some((id, end)) if *id == e.id => *end,
                _ => e.ended_at,
            };
            (e, end.max(e.started_at))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::{Duration, TimeZone};

    use super::*;
    use crate::models::TimeEntry;
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

    fn service() -> DayPlanService {
        let repo: Arc<dyn StorageRepository> = Arc::new(MemoryRepo);
        DayPlanService::new(StoreHandle::new(Arc::new(Mutex::new(AppStateStore::default())), repo))
    }

    fn local(date: NaiveDate, h: u32, m: u32) -> DateTime<Utc> {
        Local
            .from_local_datetime(&date.and_hms_opt(h, m, 0).unwrap())
            .earliest()
            .unwrap()
            .with_timezone(&Utc)
    }

    fn add_task(svc: &DayPlanService, title: &str, section: TaskSection, order: i32) -> String {
        svc.handle().mutate(|s| {
            let mut t = Task::new("l", title, 30, section);
            t.sort_order = order;
            s.add_task(t).id
        })
    }

    #[test]
    fn phase_follows_plan_skip_and_wrap_up() {
        let svc = service();
        let d = svc.today();
        assert_eq!(svc.phase(d, false), DayPhase::Empty);
        assert_eq!(svc.phase(d, true), DayPhase::Planning);
        svc.finish_planning(d, 120);
        assert_eq!(svc.phase(d, false), DayPhase::Working);
        svc.wrap_up(d, " note ");
        assert_eq!(svc.phase(d, false), DayPhase::WrappedUp);
        svc.reopen_day(d);
        assert_eq!(svc.phase(d, false), DayPhase::Working);

        let other = d.succ_opt().unwrap();
        svc.skip_planning(other);
        assert_eq!(svc.phase(other, false), DayPhase::Working);
        assert_eq!(svc.handle().read(|s| s.day_plan(d).unwrap().planning_secs), 120);
    }

    #[test]
    fn queue_lists_open_today_tasks_in_order() {
        let svc = service();
        let b = add_task(&svc, "B", TaskSection::Today, 2);
        let a = add_task(&svc, "A", TaskSection::Today, 1);
        add_task(&svc, "Later", TaskSection::Backlog, 0);
        let waiting = add_task(&svc, "W", TaskSection::Today, 3);
        svc.handle().mutate(|s| s.task_mut(&waiting).unwrap().mark_waiting());
        let ids: Vec<String> = svc.priority_tasks().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![a, b]);
    }

    #[test]
    fn planning_moves_tasks_in_and_out_of_today() {
        let svc = service();
        let id = add_task(&svc, "X", TaskSection::Backlog, 0);
        assert!(svc.priority_tasks().is_empty());
        assert!(svc.add_to_today(&id));
        assert_eq!(svc.priority_tasks().len(), 1);
        assert!(svc.remove_from_today(&id));
        assert!(svc.priority_tasks().is_empty());
        let ranked: Vec<_> = svc.candidate_tasks().into_iter().map(|t| t.section).collect();
        assert_eq!(ranked, vec![TaskSection::ThisWeek]);
        assert!(!svc.add_to_today("missing"));
    }

    #[test]
    fn goal_summary_splits_time_and_ignores_breaks() {
        let svc = service();
        let d = svc.today();
        svc.handle().mutate(|s| {
            s.preferences.daily_goal_mins = 120;
            s.time_entries.push(TimeEntry::new(TimeEntryKind::Focus, local(d, 9, 0), local(d, 9, 45)));
            s.time_entries.push(TimeEntry::new(TimeEntryKind::Planning, local(d, 8, 50), local(d, 9, 0)));
            s.time_entries.push(TimeEntry::new(TimeEntryKind::Break, local(d, 9, 45), local(d, 9, 55)));
            let mut meeting = TimeEntry::new(TimeEntryKind::Other, local(d, 11, 0), local(d, 11, 30));
            meeting.label = "Team meeting".into();
            s.time_entries.push(meeting);
        });
        let g = svc.goal_summary(d, None);
        assert_eq!(g.focus_secs, 45 * 60);
        assert_eq!(g.meeting_secs, 30 * 60);
        assert_eq!(g.other_secs, 10 * 60);
        assert_eq!(g.worked_secs, 85 * 60);
        assert_eq!(g.left_secs, 35 * 60);
        assert!((g.progress - 85.0 / 120.0).abs() < 0.001);
    }

    #[test]
    fn goal_progress_caps_at_one_and_other_days_do_not_count() {
        let svc = service();
        let d = svc.today();
        let yesterday = d.pred_opt().unwrap();
        svc.handle().mutate(|s| {
            s.preferences.daily_goal_mins = 30;
            s.time_entries.push(TimeEntry::new(TimeEntryKind::Focus, local(d, 9, 0), local(d, 10, 0)));
            s.time_entries.push(TimeEntry::new(TimeEntryKind::Focus, local(yesterday, 9, 0), local(yesterday, 10, 0)));
        });
        let g = svc.goal_summary(d, None);
        assert_eq!(g.worked_secs, 3600);
        assert_eq!(g.left_secs, 0);
        assert_eq!(g.progress, 1.0);
    }

    #[test]
    fn timeline_places_blocks_by_local_minutes_and_honours_live_end() {
        let svc = service();
        let d = svc.today();
        let live_id = svc.handle().mutate(|s| {
            s.time_entries.push(TimeEntry::new(TimeEntryKind::Planning, local(d, 8, 30), local(d, 8, 40)));
            let e = TimeEntry::new(TimeEntryKind::Focus, local(d, 9, 0), local(d, 9, 0));
            let id = e.id.clone();
            s.time_entries.push(e);
            id
        });
        let blocks = svc.timeline(d, None);
        assert_eq!(blocks.len(), 2);
        assert_eq!((blocks[0].start_mins, blocks[0].length_mins), (8 * 60 + 30, 10));
        assert_eq!(blocks[1].length_mins, 1, "an empty block still shows");

        let live = (live_id, local(d, 9, 0) + Duration::minutes(25));
        let blocks = svc.timeline(d, Some(&live));
        assert_eq!((blocks[1].start_mins, blocks[1].length_mins), (9 * 60, 25));
        assert_eq!(svc.goal_summary(d, Some(&live)).focus_secs, 25 * 60);
    }

    #[test]
    fn durations_format_for_people() {
        assert_eq!(format_duration(0), "0m");
        assert_eq!(format_duration(45 * 60 + 20), "45m");
        assert_eq!(format_duration(65 * 60), "1h 05m");
    }
}
