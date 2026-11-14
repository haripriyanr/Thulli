//! Writes an encrypted data file with synthetic goals and tasks, for visual
//! and performance checks. It never touches the real data folder: pass the
//! folder to write into, then start the app with APPDATA pointing at it.
//!
//!   cargo run --example qa_fixture -- <folder> <empty|normal|heavy|stress>

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Duration, Local, Utc};
use thulli_core::models::{
    AppStateStore, FocusSession, Goal, GoalPriority, GoalStatus, Milestone, ProgressMode, Task, TaskList, TaskSection,
    TimeEntry, TimeEntryKind,
};
use thulli_core::repository::{EncryptedStorageRepository, StorageRepository};
use thulli_core::security::SecurityManager;

fn main() {
    let mut args = std::env::args().skip(1);
    let folder = PathBuf::from(args.next().expect("folder"));
    let scenario = args.next().unwrap_or_else(|| "normal".into());
    let (goals, tasks, notes) = match scenario.as_str() {
        "empty" => (0, 0, 0),
        "normal" => (10, 100, 10),
        "heavy" => (50, 400, 50),
        "stress" => (100, 600, 300),
        other => panic!("unknown scenario {other}"),
    };

    let mut store = AppStateStore::default();
    if tasks > 0 {
        for (name, color) in [("Study", "#5B8DEF"), ("Work", "#4CC36B"), ("Home", "#E879F9")] {
            store.lists.push(TaskList::new(name, color));
        }
    }
    let today = Local::now().date_naive();
    let now = Utc::now();
    for i in 0..tasks {
        let list = store.lists[i % store.lists.len()].id.clone();
        let section = [TaskSection::Today, TaskSection::ThisWeek, TaskSection::Backlog][i % 3];
        let mut t = Task::new(list, format!("Sample task {}", i + 1), 15 + (i as u32 % 6) * 15, section);
        if i % 7 == 0 {
            t.complete();
        }
        if i % 5 == 0 {
            t.deadline = Some(today + Duration::days((i % 20) as i64 - 4));
        }
        store.tasks.push(t);
    }
    let kinds = [GoalPriority::High, GoalPriority::Medium, GoalPriority::Low];
    for g in 0..goals {
        let mut goal = Goal::new(format!("Sample goal {}", g + 1));
        goal.description = "A synthetic goal used for visual and performance checks.".into();
        goal.motivation = if g % 3 == 0 { "It keeps the rest of the week focused.".into() } else { String::new() };
        goal.category = ["Study", "Health", "Work", "Home"][g % 4].into();
        goal.priority = kinds[g % 3];
        goal.target_date = Some(today + Duration::days((g as i64 % 40) - 6));
        if g % 4 == 3 {
            goal.progress_mode = ProgressMode::Measured;
            goal.target_value = 20.0;
            goal.current_value = (g % 21) as f64;
            goal.unit = "books".into();
        }
        for m in 0..(3 + g % 4) {
            let mut ms = Milestone::new(format!("Milestone {}", m + 1));
            if m % 3 == 0 {
                ms.completed_at = Some(now);
            }
            if m % 2 == 0 {
                ms.target_date = Some(today + Duration::days(m as i64 * 7 - 5));
            }
            goal.milestones.push(ms);
        }
        goal.renumber_milestones();
        if g % 9 == 8 {
            goal.set_status(GoalStatus::Paused);
        }
        if g % 13 == 12 {
            goal.set_status(GoalStatus::Completed);
        }
        let per_goal = if tasks == 0 { 0 } else { (tasks / goals.max(1)).min(6) };
        for k in 0..per_goal {
            let idx = (g * per_goal + k) % store.tasks.len();
            store.tasks[idx].goal_id = Some(goal.id.clone());
            store.tasks[idx].milestone_id = goal.milestones.get(k % goal.milestones.len()).map(|m| m.id.clone());
            // Focus time on linked tasks.
            let task_id = store.tasks[idx].id.clone();
            for s in 0..3 {
                let start = now - Duration::days(((g + k + s) % 12) as i64) - Duration::hours(s as i64);
                let mut session = FocusSession::new(1500, Some(task_id.clone()));
                session.finish(1500);
                let mut e = TimeEntry::new(TimeEntryKind::Focus, start, start + Duration::minutes(25));
                e.task_id = Some(task_id.clone());
                e.session_id = Some(session.id.clone());
                store.focus_sessions.push(session);
                store.time_entries.push(e);
            }
        }
        store.goals.push(goal);
    }
    for n in 0..notes {
        let mut note = thulli_core::models::Note::new(format!("Sample note {}", n + 1));
        note.body = "Synthetic note body used for checks.".into();
        store.notes.push(note);
    }

    let dir = folder.join("thulli");
    std::fs::create_dir_all(&dir).expect("create folder");
    let security = Arc::new(SecurityManager::init().expect("security"));
    let repo = EncryptedStorageRepository::with_custom_path(security, dir.join("thulli_data.enc"));
    repo.save(&store).expect("save");
    println!(
        "wrote {} ({} goals, {} tasks, {} notes, {} focus entries)",
        dir.join("thulli_data.enc").display(),
        store.goals.len(),
        store.tasks.len(),
        store.notes.len(),
        store.time_entries.len()
    );
}
