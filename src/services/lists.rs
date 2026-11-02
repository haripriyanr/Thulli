//! List lifecycle: create, edit, duplicate, archive, folders and delete.

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::models::{AppStateStore, ListFolder, Task, TaskList, TaskStatus};
use crate::repository::StorageRepository;
use crate::services::store_handle::StoreHandle;

/// Image types accepted for a list icon.
pub const ICON_EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "svg"];

/// True when `path` has an extension accepted for list icons.
pub fn is_supported_icon(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ICON_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// What an edit does to a list's icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconChange {
    Keep,
    Clear,
    Set(String),
}

/// Service managing task lists and their folders.
pub struct ListService {
    handle: StoreHandle,
}

impl ListService {
    pub fn new(store: Arc<Mutex<AppStateStore>>, repository: Arc<dyn StorageRepository>) -> Self {
        Self { handle: StoreHandle::new(store, repository) }
    }

    pub fn handle(&self) -> &StoreHandle {
        &self.handle
    }

    /// Creates and persists a new custom list.
    pub fn create_list(&self, name: &str, color: &str) -> TaskList {
        let new_list = TaskList::new(name, color);
        self.handle.mutate(|st| st.add_list(new_list.clone()));
        new_list
    }

    /// Creates a list with an optional icon (empty path for none). A blank
    /// name or an unsupported icon type is rejected.
    pub fn create_list_with_icon(&self, name: &str, color: &str, icon_path: &str) -> Result<TaskList, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Give the list a name.".to_string());
        }
        let icon = icon_path.trim();
        if !icon.is_empty() && !is_supported_icon(icon) {
            return Err("Choose a jpg, png or svg image for the icon.".to_string());
        }
        let mut list = TaskList::new(name, color);
        if !icon.is_empty() {
            list = list.with_icon(icon);
        }
        self.handle.mutate(|st| {
            list.sort_order = st.lists.iter().map(|l| l.sort_order).max().map_or(0, |m| m + 1);
            st.add_list(list.clone())
        });
        Ok(list)
    }

    /// Changes a list's name, colour and icon.
    pub fn edit_list(&self, list_id: &str, name: &str, color: &str, icon: IconChange) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Give the list a name.".to_string());
        }
        if let IconChange::Set(path) = &icon {
            if !is_supported_icon(path) {
                return Err("Choose a jpg, png or svg image for the icon.".to_string());
            }
        }
        let found = self.handle.mutate(|st| match st.lists.iter_mut().find(|l| l.id == list_id) {
            Some(list) => {
                list.name = name.to_string();
                if !color.is_empty() {
                    list.color = color.to_string();
                }
                match icon {
                    IconChange::Keep => {}
                    IconChange::Clear => list.icon_path = None,
                    IconChange::Set(path) => list.icon_path = Some(path),
                }
                true
            }
            None => false,
        });
        if found { Ok(()) } else { Err("That list no longer exists.".to_string()) }
    }

    /// Copies a list with its unfinished tasks and their steps. Finished and
    /// cancelled tasks stay with the original.
    pub fn duplicate_list(&self, list_id: &str) -> Option<TaskList> {
        self.handle.mutate(|st| {
            let source = st.lists.iter().find(|l| l.id == list_id)?.clone();
            let mut copy = source.duplicate();
            copy.sort_order = st.lists.iter().map(|l| l.sort_order).max().map_or(0, |m| m + 1);

            let keep: Vec<&Task> = st
                .tasks
                .iter()
                .filter(|t| t.list_id == list_id && t.is_pending() && !has_closed_ancestor(st, t))
                .collect();
            let id_map: Vec<(String, String)> =
                keep.iter().map(|t| (t.id.clone(), uuid::Uuid::new_v4().to_string())).collect();
            let mut copies: Vec<Task> = Vec::new();
            for task in keep {
                let mut c = task.clone();
                c.id = id_map.iter().find(|(o, _)| *o == task.id).map(|(_, n)| n.clone()).unwrap_or_default();
                c.list_id = copy.id.clone();
                c.parent_id = task
                    .parent_id
                    .as_ref()
                    .and_then(|p| id_map.iter().find(|(o, _)| o == p).map(|(_, n)| n.clone()));
                c.time_taken_secs = 0;
                c.history.clear();
                c.record("Copied");
                copies.push(c);
            }
            st.add_list(copy.clone());
            st.tasks.extend(copies);
            Some(copy)
        })
    }

    /// Archives a list. Returns false when it does not exist or is already archived.
    pub fn archive_list(&self, list_id: &str) -> bool {
        self.set_archived(list_id, true)
    }

    /// Restores an archived list.
    pub fn restore_list(&self, list_id: &str) -> bool {
        self.set_archived(list_id, false)
    }

    fn set_archived(&self, list_id: &str, archived: bool) -> bool {
        self.handle.mutate(|st| {
            if archived && st.lists.iter().filter(|l| !l.is_archived).count() <= 1 && st.lists.iter().any(|l| l.id == list_id && !l.is_archived) {
                // The last active list stays so new tasks always have a home.
                return false;
            }
            match st.lists.iter_mut().find(|l| l.id == list_id) {
                Some(list) if list.is_archived != archived => {
                    list.toggle_archived();
                    true
                }
                _ => false,
            }
        })
    }

    /// Deletes a list and all associated tasks.
    pub fn delete_list(&self, list_id: &str) {
        self.handle.mutate(|st| st.remove_list_cascade(list_id));
    }

    // -- Folders ----------------------------------------------------------

    /// Creates a sidebar folder. A blank name is rejected.
    pub fn create_folder(&self, name: &str) -> Option<ListFolder> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let mut folder = ListFolder::new(name);
        self.handle.mutate(|st| {
            folder.sort_order = st.list_folders.iter().map(|f| f.sort_order).max().map_or(0, |m| m + 1);
            st.list_folders.push(folder.clone());
        });
        Some(folder)
    }

    /// Puts a list in a folder, or takes it out when `folder_id` is empty.
    pub fn move_list_to_folder(&self, list_id: &str, folder_id: &str) -> bool {
        self.handle.mutate(|st| {
            if !folder_id.is_empty() && !st.list_folders.iter().any(|f| f.id == folder_id) {
                return false;
            }
            match st.lists.iter_mut().find(|l| l.id == list_id) {
                Some(list) => {
                    list.folder_id = if folder_id.is_empty() { None } else { Some(folder_id.to_string()) };
                    true
                }
                None => false,
            }
        })
    }

    /// Expands or collapses a folder. Returns the new collapsed state.
    pub fn toggle_folder(&self, folder_id: &str) -> Option<bool> {
        self.handle.mutate(|st| st.list_folders.iter_mut().find(|f| f.id == folder_id).map(|f| f.toggle_collapsed()))
    }

    /// Deletes a folder; its lists go back to the top level.
    pub fn delete_folder(&self, folder_id: &str) -> bool {
        self.handle.mutate(|st| {
            let before = st.list_folders.len();
            st.list_folders.retain(|f| f.id != folder_id);
            for list in st.lists.iter_mut().filter(|l| l.folder_id.as_deref() == Some(folder_id)) {
                list.folder_id = None;
            }
            st.list_folders.len() < before
        })
    }

    // -- Archive ----------------------------------------------------------

    /// Finished top-level tasks, newest first, for the archive's done tab.
    pub fn archived_done_tasks(&self) -> Vec<Task> {
        self.handle.read(|st| {
            let mut done: Vec<Task> =
                st.tasks.iter().filter(|t| !t.is_step() && t.status == TaskStatus::Completed).cloned().collect();
            done.sort_by(|a, b| b.completed_at.unwrap_or(b.created_at).cmp(&a.completed_at.unwrap_or(a.created_at)));
            done
        })
    }

    /// Reopens a finished task from the archive.
    pub fn reopen_task(&self, task_id: &str) -> bool {
        self.handle.mutate(|st| st.task_mut(task_id).map(|t| t.reopen()).is_some())
    }
}

/// True when a step sits under a parent that is finished or cancelled.
fn has_closed_ancestor(st: &AppStateStore, task: &Task) -> bool {
    let mut current = task.parent_id.as_deref();
    let mut guard = 0;
    while let Some(id) = current {
        guard += 1;
        if guard > 16 {
            break;
        }
        match st.task(id) {
            Some(p) if !p.is_pending() => return true,
            Some(p) => current = p.parent_id.as_deref(),
            None => break,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{TaskSection, TaskStatus};
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

    fn service() -> ListService {
        ListService::new(Arc::new(Mutex::new(AppStateStore::with_space())), Arc::new(MemoryRepo))
    }

    #[test]
    fn icon_types_are_checked() {
        assert!(is_supported_icon("C:\\pics\\logo.PNG"));
        assert!(is_supported_icon("a.svg") && is_supported_icon("a.jpeg") && is_supported_icon("a.jpg"));
        assert!(!is_supported_icon("a.gif") && !is_supported_icon("noext"));
    }

    #[test]
    fn create_validates_name_and_icon() {
        let svc = service();
        assert!(svc.create_list_with_icon("  ", "#fff", "").is_err());
        assert!(svc.create_list_with_icon("Work", "#fff", "x.bmp").is_err());
        let l = svc.create_list_with_icon(" Work ", "#112233", "icon.png").unwrap();
        assert_eq!((l.name.as_str(), l.icon_path.as_deref()), ("Work", Some("icon.png")));
        assert_eq!(svc.handle().read(|s| s.lists.len()), 2);
    }

    #[test]
    fn edit_changes_fields_and_icon() {
        let svc = service();
        let l = svc.create_list_with_icon("Work", "#112233", "icon.png").unwrap();
        svc.edit_list(&l.id, "Office", "#445566", IconChange::Keep).unwrap();
        let got = svc.handle().read(|s| s.lists.iter().find(|x| x.id == l.id).cloned()).unwrap();
        assert_eq!((got.name.as_str(), got.color.as_str(), got.icon_path.as_deref()), ("Office", "#445566", Some("icon.png")));
        svc.edit_list(&l.id, "Office", "", IconChange::Clear).unwrap();
        assert!(svc.handle().read(|s| s.lists.iter().find(|x| x.id == l.id).unwrap().icon_path.is_none()));
        assert!(svc.edit_list(&l.id, "Office", "", IconChange::Set("a.txt".into())).is_err());
        assert!(svc.edit_list(&l.id, "", "", IconChange::Keep).is_err());
        assert!(svc.edit_list("missing", "x", "", IconChange::Keep).is_err());
    }

    #[test]
    fn duplicate_copies_unfinished_tasks_with_steps() {
        let svc = service();
        let l = svc.create_list("Work", "#112233");
        let open = svc.handle().mutate(|st| {
            let t = st.add_task(Task::new(l.id.clone(), "Open", 10, TaskSection::Today));
            let step = Task::new_step(&t, "Step");
            st.add_task(step);
            let mut done = Task::new(l.id.clone(), "Done", 10, TaskSection::Today);
            done.complete();
            st.add_task(done);
            t
        });
        let copy = svc.duplicate_list(&l.id).unwrap();
        assert_eq!(copy.name, "Work copy");
        let (copied, original_count) = svc.handle().read(|st| {
            (
                st.tasks.iter().filter(|t| t.list_id == copy.id).cloned().collect::<Vec<_>>(),
                st.tasks.iter().filter(|t| t.list_id == l.id).count(),
            )
        });
        assert_eq!(original_count, 3);
        assert_eq!(copied.len(), 2);
        let new_parent = copied.iter().find(|t| t.title == "Open").unwrap();
        let new_step = copied.iter().find(|t| t.title == "Step").unwrap();
        assert_ne!(new_parent.id, open.id);
        assert_eq!(new_step.parent_id.as_deref(), Some(new_parent.id.as_str()));
        assert!(svc.duplicate_list("missing").is_none());
    }

    #[test]
    fn archive_and_restore_round_trip_but_keep_one_active_list() {
        let svc = service();
        let personal = svc.handle().read(|s| s.lists[0].id.clone());
        assert!(!svc.archive_list(&personal), "last active list stays");
        let l = svc.create_list("Work", "#112233");
        assert!(svc.archive_list(&l.id));
        assert!(!svc.archive_list(&l.id), "already archived");
        assert!(svc.handle().read(|s| s.lists.iter().find(|x| x.id == l.id).unwrap().archived_at.is_some()));
        assert!(svc.restore_list(&l.id));
        assert!(!svc.restore_list(&l.id));
        assert!(!svc.archive_list("missing"));
    }

    #[test]
    fn folders_group_lists() {
        let svc = service();
        assert!(svc.create_folder("  ").is_none());
        let folder = svc.create_folder("Work stuff").unwrap();
        let l = svc.create_list("A", "#fff");
        assert!(svc.move_list_to_folder(&l.id, &folder.id));
        assert!(!svc.move_list_to_folder(&l.id, "ghost"));
        assert_eq!(svc.toggle_folder(&folder.id), Some(true));
        assert_eq!(svc.toggle_folder(&folder.id), Some(false));
        assert!(svc.toggle_folder("ghost").is_none());
        assert!(svc.delete_folder(&folder.id));
        assert!(svc.handle().read(|s| s.lists.iter().find(|x| x.id == l.id).unwrap().folder_id.is_none()));
        assert!(svc.move_list_to_folder(&l.id, ""));
    }

    #[test]
    fn archive_lists_finished_tasks_and_can_reopen() {
        let svc = service();
        let id = svc.handle().mutate(|st| {
            let mut t = Task::new("l", "Finished", 5, TaskSection::Today);
            t.complete();
            st.add_task(t).id
        });
        svc.handle().mutate(|st| {
            st.add_task(Task::new("l", "Open", 5, TaskSection::Today));
        });
        let done = svc.archived_done_tasks();
        assert_eq!(done.len(), 1);
        assert!(svc.reopen_task(&id));
        assert!(svc.archived_done_tasks().is_empty());
        assert_eq!(svc.handle().read(|s| s.task(&id).unwrap().status), TaskStatus::Open);
    }

    #[test]
    fn delete_cascades_tasks() {
        let svc = service();
        let l = svc.create_list("Work", "#112233");
        svc.handle().mutate(|st| {
            st.add_task(Task::new(l.id.clone(), "x", 5, TaskSection::Today));
        });
        svc.delete_list(&l.id);
        assert!(svc.handle().read(|s| s.tasks.is_empty() && s.lists.len() == 1));
    }
}
