//! Conversions from domain models to the Slint structs in `ui/types.slint`.
//!
//! Shared by several screens. Add converters for new structs in your own
//! screen file instead of growing this one.

use std::rc::Rc;

use chrono::Local;
use slint::{Color, Image, ModelRc, VecModel};
use thulli_core::models::{AppStateStore, BlockedApp, BlockedSite, Task, TaskList, TaskStatus};

use crate::{BlockedAppModel, BlockedSiteModel, ListModel, TagModel, TaskModel};

/// Parses `#RRGGBB`; falls back to a neutral grey on bad input.
pub fn hex_color(hex: &str) -> Color {
    let h = hex.trim().trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8);
        }
    }
    Color::from_rgb_u8(0x9E, 0xAB, 0xBE)
}

/// Nesting level of a task (0 for top-level).
fn depth_of(store: &AppStateStore, task: &Task) -> i32 {
    let mut depth = 0;
    let mut current = task.parent_id.as_deref();
    while let Some(parent_id) = current {
        depth += 1;
        if depth > 16 {
            break;
        }
        current = store.task(parent_id).and_then(|p| p.parent_id.as_deref());
    }
    depth
}

pub fn tag_model(tag: &thulli_core::models::Tag) -> TagModel {
    TagModel {
        id: tag.id.clone().into(),
        name: tag.name.clone().into(),
        emoji: tag.emoji.clone().unwrap_or_default().into(),
        color: hex_color(&tag.color),
    }
}

pub fn task_model(store: &AppStateStore, task: &Task) -> TaskModel {
    let today = Local::now().date_naive();
    let tags: Vec<TagModel> = task
        .tags
        .iter()
        .filter_map(|id| store.tags.iter().find(|t| &t.id == id))
        .map(tag_model)
        .collect();
    let list = store.lists.iter().find(|l| l.id == task.list_id);
    let steps = store.steps_of(&task.id);
    TaskModel {
        id: task.id.clone().into(),
        list_id: task.list_id.clone().into(),
        list_name: list.map(|l| l.name.clone()).unwrap_or_default().into(),
        list_color: hex_color(list.map(|l| l.color.as_str()).unwrap_or("")),
        parent_id: task.parent_id.clone().unwrap_or_default().into(),
        title: task.title.clone().into(),
        notes: task.notes.clone().into(),
        estimate_mins: task.estimate_mins as i32,
        tracked_mins: (task.time_taken_secs / 60) as i32,
        status: task.status.as_str().into(),
        section: task.section.as_str().into(),
        quadrant: task.matrix_quadrant.map(|q| q.as_str()).unwrap_or_default().into(),
        tags: ModelRc::from(Rc::new(VecModel::from(tags))),
        do_date_label: task.do_date.map(|d| d.format("%a %d %b").to_string()).unwrap_or_default().into(),
        deadline_label: task.deadline.map(|d| d.format("%a %d %b").to_string()).unwrap_or_default().into(),
        overdue: task.is_pending() && task.deadline.is_some_and(|d| d < today),
        over_estimate: task.is_over_estimate(),
        depth: depth_of(store, task),
        step_count: steps.len() as i32,
        steps_done: steps.iter().filter(|s| s.status == TaskStatus::Completed).count() as i32,
        scheduled_label: task
            .scheduled_at
            .map(|d| d.with_timezone(&Local).format("%a %d %b, %H:%M").to_string())
            .unwrap_or_default()
            .into(),
        selected: false,
    }
}

pub fn list_model(store: &AppStateStore, list: &TaskList) -> ListModel {
    let icon = list.icon_path.as_deref().and_then(|p| Image::load_from_path(std::path::Path::new(p)).ok());
    ListModel {
        id: list.id.clone().into(),
        name: list.name.clone().into(),
        color: hex_color(&list.color),
        badge_letter: list.badge_letter().into(),
        folder_id: list.folder_id.clone().unwrap_or_default().into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        pending_count: store.count_pending_for_list(&list.id) as i32,
        is_archived: list.is_archived,
    }
}

pub fn blocked_app_model(app: &BlockedApp) -> BlockedAppModel {
    BlockedAppModel {
        id: app.id.clone().into(),
        name: app.name.clone().into(),
        process_name: app.process_name.clone().into(),
        active: app.active,
    }
}

pub fn blocked_site_model(site: &BlockedSite) -> BlockedSiteModel {
    BlockedSiteModel { id: site.id.clone().into(), domain: site.domain.clone().into(), active: site.active }
}
