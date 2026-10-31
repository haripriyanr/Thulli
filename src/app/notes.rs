//! Wiring for the notes screen (ui/screens/notes.slint) and the floating
//! notes / quick capture window.
//!
//! Editing logic lives in `thulli_core::services::notes` (`NoteEditor`); this
//! file only moves data between that service and the Slint globals.
//!
//! Public entry points for other slices (all UI-thread unless noted):
//! * [`toggle_floating_notes`] / [`toggle_quick_capture`] show or hide the
//!   always-on-top window.
//! * [`request_toggle_floating_notes`] / [`request_toggle_quick_capture`] are
//!   safe to call from any thread (for example a global hotkey listener).
//! * [`open_task_notes`] opens (or creates) the notes for a task on the Notes screen.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use chrono::Local;
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use thulli_core::models::{MeetingNote, Note};
use thulli_core::services::notes::{
    preview_of, slash_commands, stamp_utc, BlockKind, EditEffect, NoteEditor, NoteScope,
};
use thulli_core::window::WindowManager;

use crate::app::context::AppContext;
use crate::{
    BragModel, ChoiceModel, FloatingNotesWindow, MainWindow, MeetingNoteModel, NoteBlockModel, NoteFolderModel,
    NoteModel, NotesState, QuickCaptureModel, ShellState, SlashItemModel,
};

const SAVE_DELAY: Duration = Duration::from_millis(600);
const MESSAGE_TIME: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Note,
    Meeting,
}

/// The item currently open in the editor.
struct Session {
    kind: Kind,
    id: String,
    title: String,
    editor: NoteEditor,
    dirty: bool,
}

struct Inner {
    window: slint::Weak<MainWindow>,
    ctx: AppContext,
    session: RefCell<Option<Session>>,
    save_timer: Timer,
    message_timer: Timer,
    // Floating window
    floating: RefCell<Option<FloatingNotesWindow>>,
    float_note: RefCell<Option<(String, String)>>, // note id, body
    float_timer: Timer,
}

thread_local! {
    static NOTES: RefCell<Option<Rc<Inner>>> = const { RefCell::new(None) };
}

fn with_inner<R>(f: impl FnOnce(&Rc<Inner>) -> R) -> Option<R> {
    NOTES.with(|n| n.borrow().as_ref().map(f))
}

/// Shows or hides the floating notes window (notes for the running task).
pub fn toggle_floating_notes() {
    with_inner(|i| i.toggle_floating("notes"));
}

/// Shows or hides the floating Quick Capture window.
pub fn toggle_quick_capture() {
    with_inner(|i| i.toggle_floating("capture"));
}

/// Thread-safe version of [`toggle_floating_notes`].
#[allow(dead_code)] // entry point for the hotkey / tasks wiring, not yet bound in main.rs
pub fn request_toggle_floating_notes() {
    let _ = slint::invoke_from_event_loop(toggle_floating_notes);
}

/// Thread-safe version of [`toggle_quick_capture`].
#[allow(dead_code)] // entry point for the hotkey / tasks wiring, not yet bound in main.rs
pub fn request_toggle_quick_capture() {
    let _ = slint::invoke_from_event_loop(toggle_quick_capture);
}

/// Opens the notes for a task on the Notes screen, creating a note if needed.
#[allow(dead_code)] // entry point for the hotkey / tasks wiring, not yet bound in main.rs
pub fn open_task_notes(task_id: &str) {
    with_inner(|i| {
        if let Some(w) = i.window.upgrade() {
            w.global::<ShellState>().set_current_screen("notes".into());
        }
        if let Some(note) = i.ctx.notes.note_for_task(task_id) {
            i.flush();
            i.select("all");
            i.open_note(&note.id);
        }
    });
}

pub fn bind(window: &MainWindow, ctx: &AppContext) {
    let inner = Rc::new(Inner {
        window: window.as_weak(),
        ctx: ctx.clone(),
        session: RefCell::new(None),
        save_timer: Timer::default(),
        message_timer: Timer::default(),
        floating: RefCell::new(None),
        float_note: RefCell::new(None),
        float_timer: Timer::default(),
    });
    NOTES.with(|n| *n.borrow_mut() = Some(inner.clone()));

    let state = window.global::<NotesState>();
    inner.refresh_lists();

    macro_rules! on {
        ($method:ident, $inner:ident, $body:expr) => {{
            let $inner = inner.clone();
            state.$method($body);
        }};
    }

    on!(on_select_section, i, move |s| i.select_and_open(s.as_str()));
    on!(on_new_note, i, move || i.new_note());
    on!(on_new_folder, i, move |name| i.new_folder(name.as_str()));
    on!(on_open_note, i, move |id| {
        i.flush();
        i.open_note(id.as_str());
    });
    on!(on_open_meeting, i, move |id| {
        i.flush();
        i.open_meeting(id.as_str());
    });
    on!(on_trash_note, i, move |id| i.trash(id.as_str()));
    on!(on_restore_note, i, move |id| i.restore(id.as_str()));
    on!(on_delete_note_forever, i, move |id| {
        i.ctx.notes.delete_note_forever(id.as_str());
        i.close_item();
        i.refresh_lists();
    });
    on!(on_empty_trash, i, move || {
        i.ctx.notes.empty_trash();
        i.close_item();
        i.refresh_lists();
    });
    on!(on_delete_folder, i, move |id| i.delete_folder(id.as_str()));
    on!(on_move_note, i, move |id, folder| {
        let folder = (!folder.is_empty()).then(|| folder.to_string());
        i.ctx.notes.move_note(id.as_str(), folder.as_deref());
        i.reload_header();
        i.refresh_lists();
    });
    on!(on_link_task, i, move |task| {
        let Some(note_id) = i.open_note_id() else { return };
        let task = (!task.is_empty()).then(|| task.to_string());
        i.ctx.notes.set_note_task(&note_id, task.as_deref());
        i.reload_header();
    });
    on!(on_prepare_pickers, i, move || i.prepare_pickers());
    on!(on_title_edited, i, move |t| i.title_edited(t.as_str()));
    on!(on_save_note, i, move |id, title, body| {
        i.ctx.notes.update_note(id.as_str(), title.as_str(), body.as_str());
        i.refresh_lists();
    });

    on!(on_block_edited, i, move |idx, text| i.block_edited(idx as usize, text.as_str()));
    on!(on_block_enter, i, move |idx, cursor| i.edit(|e| e.enter(idx as usize, cursor.max(0) as usize)));
    on!(on_block_backspace_start, i, move |idx| i.backspace_start(idx as usize));
    on!(on_block_indent, i, move |idx| i.edit(|e| e.indent(idx as usize)));
    on!(on_block_outdent, i, move |idx| i.edit(|e| e.outdent(idx as usize)));
    on!(on_block_toggle_todo, i, move |idx| i.edit(|e| e.toggle_todo(idx as usize)));
    on!(on_block_checked, i, move |idx, checked| {
        i.with_session(|s| s.editor.set_checked(idx as usize, checked));
        i.push_blocks(EditEffect { focus: None, structural: true });
        i.mark_dirty();
    });
    on!(on_slash_apply, i, move |idx, id| {
        i.clear_slash();
        i.edit(|e| e.apply_slash(idx as usize, id.as_str()));
    });
    on!(on_slash_dismiss, i, move || i.clear_slash());
    on!(on_block_task_from_selection, i, move |idx, a, c| i.task_from_selection(idx as usize, a, c));
    on!(on_block_link_request, i, move |idx, a, c| i.request_link(idx as usize, a, c));
    on!(on_block_add_link, i, move |idx, text, url| {
        i.window_do(|w| w.global::<NotesState>().set_link_block(-1));
        i.edit(|e| e.add_link(idx as usize, text.as_str(), url.as_str()));
    });
    on!(on_task_from_selection, i, move |text| i.create_task(text.as_str()));

    on!(on_add_capture, i, move |text| {
        i.ctx.notes.add_capture(text.as_str());
        i.refresh_lists();
        i.refresh_float_captures();
    });
    on!(on_capture_to_task, i, move |id| i.capture_to_task(id.as_str()));
    on!(on_capture_to_note, i, move |id| {
        if let Some(note) = i.ctx.notes.capture_to_note(id.as_str()) {
            i.say("Saved as a note");
            i.refresh_lists();
            i.refresh_float_captures();
            let _ = note;
        }
    });
    on!(on_delete_capture, i, move |id| {
        i.ctx.notes.delete_capture(id.as_str());
        i.refresh_lists();
        i.refresh_float_captures();
    });
    on!(on_start_meeting, i, move || i.start_meeting());
    on!(on_end_meeting, i, move |id| {
        i.flush();
        i.ctx.notes.end_meeting(id.as_str());
        i.reload_header();
    });
    on!(on_delete_meeting, i, move |id| {
        i.ctx.notes.delete_meeting(id.as_str());
        i.close_item();
        i.refresh_lists();
    });
    on!(on_add_brag, i, move |text| {
        i.ctx.notes.add_brag(text.as_str());
        i.refresh_lists();
    });
    on!(on_delete_brag, i, move |id| {
        i.ctx.notes.delete_brag(id.as_str());
        i.refresh_lists();
    });
    on!(on_open_floating_notes, _i, move || toggle_floating_notes());
    on!(on_open_quick_capture, _i, move || toggle_quick_capture());
}

/// Substring of `text` between two byte offsets (in either order), snapped to
/// character boundaries. Empty when the offsets are equal.
fn slice_between(text: &str, a: i32, b: i32) -> String {
    let (lo, hi) = (a.min(b).max(0) as usize, a.max(b).max(0) as usize);
    let snap = |mut at: usize| {
        at = at.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    text[snap(lo)..snap(hi)].to_string()
}

fn ss(s: impl Into<String>) -> SharedString {
    s.into().into()
}

fn model_of<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

impl Inner {
    fn window_do<R>(&self, f: impl FnOnce(&MainWindow) -> R) -> Option<R> {
        self.window.upgrade().map(|w| f(&w))
    }

    fn state<R>(&self, f: impl FnOnce(&NotesState) -> R) -> Option<R> {
        self.window_do(|w| f(&w.global::<NotesState>()))
    }

    fn with_session<R>(&self, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        self.session.borrow_mut().as_mut().map(f)
    }

    fn open_note_id(&self) -> Option<String> {
        self.session.borrow().as_ref().filter(|s| s.kind == Kind::Note).map(|s| s.id.clone())
    }

    fn section(&self) -> String {
        self.state(|s| s.get_section().to_string()).unwrap_or_default()
    }

    fn say(self: &Rc<Self>, text: &str) {
        self.state(|s| s.set_message(text.into()));
        let me = self.clone();
        self.message_timer.start(TimerMode::SingleShot, MESSAGE_TIME, move || {
            me.state(|s| s.set_message("".into()));
        });
    }

    // Lists ---------------------------------------------------------------

    fn refresh_lists(&self) {
        let svc = &self.ctx.notes;
        let section = self.section();
        let notes: Vec<NoteModel> = match NoteScope::from_section(&section) {
            Some(scope) => svc.notes_in(&scope).iter().map(|n| self.note_model(n)).collect(),
            None => Vec::new(),
        };
        let folders: Vec<NoteFolderModel> = svc
            .folders()
            .into_iter()
            .map(|f| NoteFolderModel {
                note_count: svc.folder_note_count(&f.id) as i32,
                id: ss(f.id),
                name: ss(f.name),
            })
            .collect();
        let captures: Vec<QuickCaptureModel> = svc
            .captures()
            .into_iter()
            .map(|c| QuickCaptureModel {
                id: ss(c.id),
                text: ss(c.text),
                created_label: ss(stamp_utc(c.created_at)),
                processed: c.processed,
            })
            .collect();
        let meetings: Vec<MeetingNoteModel> = svc.meetings().iter().map(meeting_model).collect();
        let brags: Vec<BragModel> = svc
            .brags()
            .into_iter()
            .map(|b| BragModel { id: ss(b.id), text: ss(b.text), date_label: ss(b.date.format("%a, %b %d").to_string()) })
            .collect();
        let all = svc.notes_in(&NoteScope::All).len() as i32;
        let unsorted = svc.notes_in(&NoteScope::Unsorted).len() as i32;
        let trash = svc.notes_in(&NoteScope::Trash).len() as i32;

        self.state(|s| {
            s.set_notes(model_of(notes));
            s.set_folders(model_of(folders));
            s.set_captures(model_of(captures));
            s.set_meetings(model_of(meetings));
            s.set_brags(model_of(brags));
            s.set_all_count(all);
            s.set_unsorted_count(unsorted);
            s.set_trash_count(trash);
        });
    }

    fn note_model(&self, n: &Note) -> NoteModel {
        NoteModel {
            id: ss(n.id.clone()),
            folder_id: ss(n.folder_id.clone().unwrap_or_default()),
            title: ss(n.title.clone()),
            preview: ss(preview_of(&n.body)),
            body: ss(n.body.clone()),
            updated_label: ss(stamp_utc(n.updated_at)),
            backlink_count: self.ctx.notes.backlink_count(n) as i32,
            trashed: n.is_trashed(),
        }
    }

    fn prepare_pickers(&self) {
        let mut folders = vec![ChoiceModel { id: "".into(), label: "Unsorted".into() }];
        folders.extend(self.ctx.notes.folders().into_iter().map(|f| ChoiceModel { id: ss(f.id), label: ss(f.name) }));
        let mut tasks = vec![ChoiceModel { id: "".into(), label: "No task".into() }];
        tasks.extend(self.ctx.notes.linkable_tasks(12).into_iter().map(|(id, t)| ChoiceModel { id: ss(id), label: ss(t) }));
        self.state(|s| {
            s.set_folder_choices(model_of(folders));
            s.set_task_choices(model_of(tasks));
        });
    }

    // Sections ------------------------------------------------------------

    fn select(&self, section: &str) {
        self.close_item();
        self.state(|s| s.set_section(section.into()));
        self.refresh_lists();
    }

    /// Switches section and opens its first item, if it has one.
    fn select_and_open(self: &Rc<Self>, section: &str) {
        self.flush();
        self.select(section);
        let first = self.state(|s| s.get_notes().row_data(0).map(|n| n.id.to_string())).flatten();
        match section {
            "meetings" => {
                if let Some(m) = self.ctx.notes.meetings().first() {
                    self.open_meeting(&m.id);
                }
            }
            _ => {
                if let Some(id) = first {
                    self.open_note(&id);
                }
            }
        }
    }

    fn new_folder(self: &Rc<Self>, name: &str) {
        if let Some(folder) = self.ctx.notes.create_folder(name) {
            self.select_and_open(&folder.id);
        }
    }

    fn delete_folder(self: &Rc<Self>, id: &str) {
        self.flush();
        self.ctx.notes.delete_folder(id);
        if self.section() == id {
            self.select_and_open("all");
        } else {
            self.refresh_lists();
        }
    }

    // Open item -----------------------------------------------------------

    fn close_item(&self) {
        *self.session.borrow_mut() = None;
        self.state(|s| {
            s.set_open_kind("".into());
            s.set_open_note_id("".into());
            s.set_blocks(model_of(Vec::new()));
            s.set_slash_block(-1);
            s.set_link_block(-1);
        });
    }

    fn open_note(self: &Rc<Self>, id: &str) {
        let Some(note) = self.ctx.notes.note(id) else { return };
        let session = Session {
            kind: Kind::Note,
            id: note.id.clone(),
            title: note.title.clone(),
            editor: NoteEditor::from_body(&note.body),
            dirty: false,
        };
        *self.session.borrow_mut() = Some(session);
        self.state(|s| {
            s.set_open_kind("note".into());
            s.set_open_note_id(ss(note.id.clone()));
            s.set_link_block(-1);
            s.set_slash_block(-1);
        });
        self.reload_header();
        self.push_blocks(EditEffect { focus: None, structural: true });
    }

    fn open_meeting(self: &Rc<Self>, id: &str) {
        let Some(m) = self.ctx.notes.meeting(id) else { return };
        let session = Session {
            kind: Kind::Meeting,
            id: m.id.clone(),
            title: m.title.clone(),
            editor: NoteEditor::from_body(&m.body),
            dirty: false,
        };
        *self.session.borrow_mut() = Some(session);
        self.state(|s| {
            s.set_open_kind("meeting".into());
            s.set_open_note_id(ss(m.id.clone()));
            s.set_link_block(-1);
            s.set_slash_block(-1);
        });
        self.reload_header();
        self.push_blocks(EditEffect { focus: None, structural: true });
    }

    /// Refreshes title, timestamp, backlinks, folder and task for the open item.
    fn reload_header(&self) {
        let Some((kind, id)) = self.session.borrow().as_ref().map(|s| (s.kind, s.id.clone())) else { return };
        match kind {
            Kind::Note => {
                let Some(n) = self.ctx.notes.note(&id) else { return };
                let folder = n
                    .folder_id
                    .as_ref()
                    .and_then(|fid| self.ctx.notes.folders().into_iter().find(|f| &f.id == fid))
                    .map(|f| f.name)
                    .unwrap_or_else(|| "Unsorted".to_string());
                let task = n.task_id.as_deref().and_then(|t| self.ctx.notes.task_title(t)).unwrap_or_default();
                let backlinks = self.ctx.notes.backlink_count(&n) as i32;
                self.state(|s| {
                    s.set_open_title(ss(n.title.clone()));
                    s.set_open_stamp(ss(format!("Edited {}", stamp_utc(n.updated_at))));
                    s.set_open_backlinks(backlinks);
                    s.set_open_task_title(ss(task));
                    s.set_open_in_trash(n.is_trashed());
                    s.set_open_folder_name(ss(folder));
                });
            }
            Kind::Meeting => {
                let Some(m) = self.ctx.notes.meeting(&id) else { return };
                let stamp = match m.ended_at {
                    Some(_) => format!("Started {}, ended", stamp_utc(m.started_at)),
                    None => format!("Started {}", stamp_utc(m.started_at)),
                };
                self.state(|s| {
                    s.set_open_title(ss(m.title.clone()));
                    s.set_open_stamp(ss(stamp));
                    s.set_open_backlinks(0);
                    s.set_open_task_title("".into());
                    s.set_open_in_trash(false);
                    s.set_open_ended(m.ended_at.is_some());
                });
            }
        }
    }

    // Editing -------------------------------------------------------------

    fn push_blocks(&self, effect: EditEffect) {
        let blocks: Vec<NoteBlockModel> = match self.session.borrow().as_ref() {
            Some(s) => s
                .editor
                .blocks()
                .iter()
                .enumerate()
                .map(|(i, b)| NoteBlockModel {
                    kind: b.kind.as_str().into(),
                    marker: ss(s.editor.marker(i)),
                    text: ss(b.text.clone()),
                    url: ss(b.url.clone()),
                    checked: b.checked,
                    indent: b.indent as i32,
                })
                .collect(),
            None => Vec::new(),
        };
        self.state(|s| {
            if let Some(f) = effect.focus {
                s.set_focus_block(f as i32);
                s.set_focus_serial(s.get_focus_serial() + 1);
            }
            s.set_blocks(model_of(blocks));
        });
    }

    /// Runs a structural edit, then rebuilds rows, moves the cursor and saves later.
    fn edit(self: &Rc<Self>, f: impl FnOnce(&mut NoteEditor) -> EditEffect) {
        let effect = self.with_session(|s| f(&mut s.editor));
        let Some(effect) = effect else { return };
        if effect.structural {
            self.state(|s| s.set_focus_cursor(0));
            self.push_blocks(effect);
            self.mark_dirty();
        }
    }

    fn block_edited(self: &Rc<Self>, index: usize, text: &str) {
        let (effect, slash) = {
            let mut guard = self.session.borrow_mut();
            let Some(s) = guard.as_mut() else { return };
            let effect = s.editor.set_text(index, text);
            let plain = s.editor.block(index).is_some_and(|b| b.kind == BlockKind::Paragraph);
            let slash = if plain && text.starts_with('/') && !text.contains(' ') { Some(slash_commands(&text[1..])) } else { None };
            (effect, slash)
        };
        match slash {
            Some(cmds) if !cmds.is_empty() => {
                let items: Vec<SlashItemModel> =
                    cmds.iter().map(|c| SlashItemModel { id: c.id.into(), label: c.label.into(), hint: c.hint.into() }).collect();
                self.state(|s| {
                    s.set_slash_items(model_of(items));
                    s.set_slash_block(index as i32);
                });
            }
            _ => self.clear_slash(),
        }
        if effect.structural {
            self.state(|s| s.set_focus_cursor(-1));
            self.push_blocks(effect);
        }
        self.mark_dirty();
    }

    fn clear_slash(&self) {
        self.state(|s| s.set_slash_block(-1));
    }

    fn backspace_start(self: &Rc<Self>, index: usize) -> bool {
        let (effect, join) = {
            let mut guard = self.session.borrow_mut();
            let Some(s) = guard.as_mut() else { return false };
            let before = s.editor.blocks().len();
            let prev_len = index.checked_sub(1).and_then(|p| s.editor.block(p)).map_or(0, |b| b.text.len());
            let Some(effect) = s.editor.backspace_at_start(index) else { return false };
            let merged = s.editor.blocks().len() < before && index > 0;
            (effect, if merged { prev_len as i32 } else { 0 })
        };
        self.state(|s| s.set_focus_cursor(join));
        self.push_blocks(effect);
        self.mark_dirty();
        true
    }

    fn request_link(&self, index: usize, anchor: i32, cursor: i32) {
        let text = self.session.borrow().as_ref().and_then(|s| s.editor.block(index).map(|b| b.text.clone()));
        let Some(text) = text else { return };
        let selected = slice_between(&text, anchor, cursor);
        let title = if selected.trim().is_empty() { text } else { selected };
        self.state(|s| {
            s.set_link_text(ss(title));
            s.set_link_block(index as i32);
        });
    }

    fn task_from_selection(self: &Rc<Self>, index: usize, anchor: i32, cursor: i32) {
        let text = self.session.borrow().as_ref().and_then(|s| s.editor.block(index).map(|b| b.text.clone()));
        let Some(text) = text else { return };
        let selected = slice_between(&text, anchor, cursor);
        let source = if selected.trim().is_empty() { text } else { selected };
        self.create_task(&source);
    }

    fn create_task(self: &Rc<Self>, text: &str) {
        match self.ctx.notes.create_task_from_text(text) {
            Some(task) => self.say(&format!("Task added: {}", task.title)),
            None => self.say("Nothing to turn into a task"),
        }
    }

    // Saving --------------------------------------------------------------

    fn mark_dirty(self: &Rc<Self>) {
        self.with_session(|s| s.dirty = true);
        let me = self.clone();
        self.save_timer.start(TimerMode::SingleShot, SAVE_DELAY, move || me.flush());
    }

    fn title_edited(self: &Rc<Self>, title: &str) {
        self.with_session(|s| s.title = title.to_string());
        self.mark_dirty();
    }

    /// Writes unsaved edits now.
    fn flush(&self) {
        self.save_timer.stop();
        let pending = {
            let mut guard = self.session.borrow_mut();
            match guard.as_mut() {
                Some(s) if s.dirty => {
                    s.dirty = false;
                    Some((s.kind, s.id.clone(), s.title.clone(), s.editor.to_body()))
                }
                _ => None,
            }
        };
        let Some((kind, id, title, body)) = pending else { return };
        match kind {
            Kind::Note => {
                self.ctx.notes.update_note(&id, &title, &body);
            }
            Kind::Meeting => {
                self.ctx.notes.update_meeting(&id, &title, &body);
            }
        }
        self.refresh_lists();
        self.reload_header();
    }

    // Actions -------------------------------------------------------------

    fn new_note(self: &Rc<Self>) {
        self.flush();
        let section = self.section();
        let folder = match NoteScope::from_section(&section) {
            Some(NoteScope::Folder(id)) => Some(id),
            _ => None,
        };
        let note = self.ctx.notes.create_note(folder.as_deref());
        if matches!(NoteScope::from_section(&section), Some(NoteScope::Trash) | None) {
            self.select("all");
        }
        self.refresh_lists();
        self.open_note(&note.id);
        self.state(|s| {
            s.set_focus_cursor(0);
            s.set_focus_block(0);
            s.set_focus_serial(s.get_focus_serial() + 1);
        });
    }

    fn trash(self: &Rc<Self>, id: &str) {
        self.flush();
        self.ctx.notes.trash_note(id);
        self.close_item();
        self.refresh_lists();
        self.say("Moved to Trash");
    }

    fn restore(self: &Rc<Self>, id: &str) {
        self.ctx.notes.restore_note(id);
        self.close_item();
        self.refresh_lists();
        self.say("Note restored");
    }

    fn capture_to_task(self: &Rc<Self>, id: &str) {
        match self.ctx.notes.capture_to_task(id) {
            Some(task) => self.say(&format!("Task added: {}", task.title)),
            None => self.say("Nothing to turn into a task"),
        }
        self.refresh_lists();
        self.refresh_float_captures();
    }

    fn start_meeting(self: &Rc<Self>) {
        self.flush();
        let title = format!("Meeting {}", Local::now().format("%b %d, %H:%M"));
        let m: MeetingNote = self.ctx.notes.start_meeting(&title);
        self.refresh_lists();
        self.open_meeting(&m.id);
    }

    // Floating window -----------------------------------------------------

    fn toggle_floating(self: &Rc<Self>, mode: &str) {
        if self.floating.borrow().is_none() {
            self.build_floating();
        }
        let guard = self.floating.borrow();
        let Some(win) = guard.as_ref() else { return };
        let showing = win.window().is_visible();
        if showing && win.get_mode() == mode {
            drop(guard);
            self.flush_float();
            if let Some(w) = self.floating.borrow().as_ref() {
                let _ = w.hide();
            }
            return;
        }
        win.set_mode(mode.into());
        self.refresh_floating_note(win);
        self.refresh_float_captures();
        place_at_right_edge(win);
        let _ = win.show();
    }

    fn build_floating(self: &Rc<Self>) {
        let Ok(win) = FloatingNotesWindow::new() else { return };
        {
            let me = self.clone();
            win.on_capture_added(move |text| {
                me.ctx.notes.add_capture(text.as_str());
                me.refresh_lists();
                me.refresh_float_captures();
            });
        }
        {
            let me = self.clone();
            win.on_capture_to_task(move |id| me.capture_to_task(id.as_str()));
        }
        {
            let me = self.clone();
            win.on_capture_deleted(move |id| {
                me.ctx.notes.delete_capture(id.as_str());
                me.refresh_lists();
                me.refresh_float_captures();
            });
        }
        {
            let me = self.clone();
            win.on_body_edited(move |body| {
                if let Some((_, b)) = me.float_note.borrow_mut().as_mut() {
                    *b = body.to_string();
                }
                let again = me.clone();
                me.float_timer.start(TimerMode::SingleShot, SAVE_DELAY, move || again.flush_float());
            });
        }
        {
            let me = self.clone();
            win.on_close_requested(move || {
                me.flush_float();
                if let Some(w) = me.floating.borrow().as_ref() {
                    let _ = w.hide();
                }
            });
        }
        win.on_drag_started(WindowManager::begin_native_drag);
        *self.floating.borrow_mut() = Some(win);
    }

    fn refresh_floating_note(&self, win: &FloatingNotesWindow) {
        self.flush_float();
        let task_id = self.ctx.timer.snapshot().task_id;
        let note = task_id.as_deref().and_then(|t| self.ctx.notes.note_for_task(t));
        match (task_id, note) {
            (Some(tid), Some(note)) => {
                win.set_task_title(ss(self.ctx.notes.task_title(&tid).unwrap_or_default()));
                win.set_note_title(ss(note.title.clone()));
                win.set_note_body(ss(note.body.clone()));
                *self.float_note.borrow_mut() = Some((note.id, note.body));
            }
            _ => {
                win.set_task_title("".into());
                win.set_note_title("".into());
                win.set_note_body("".into());
                *self.float_note.borrow_mut() = None;
            }
        }
    }

    fn flush_float(&self) {
        self.float_timer.stop();
        let pending = self.float_note.borrow().clone();
        if let Some((id, body)) = pending {
            if let Some(note) = self.ctx.notes.note(&id) {
                if note.body != body {
                    self.ctx.notes.update_note(&id, &note.title, &body);
                    // The main screen may be showing this note.
                    let open = self.open_note_id();
                    if open.as_deref() == Some(id.as_str()) {
                        let mut guard = self.session.borrow_mut();
                        if let Some(s) = guard.as_mut() {
                            s.editor = NoteEditor::from_body(&body);
                            s.dirty = false;
                        }
                        drop(guard);
                        self.push_blocks(EditEffect { focus: None, structural: true });
                    }
                    self.refresh_lists();
                }
            }
        }
    }

    fn refresh_float_captures(&self) {
        let guard = self.floating.borrow();
        let Some(win) = guard.as_ref() else { return };
        let captures: Vec<QuickCaptureModel> = self
            .ctx
            .notes
            .captures()
            .into_iter()
            .map(|c| QuickCaptureModel {
                id: ss(c.id),
                text: ss(c.text),
                created_label: ss(stamp_utc(c.created_at)),
                processed: c.processed,
            })
            .collect();
        win.set_captures(model_of(captures));
    }
}

fn meeting_model(m: &MeetingNote) -> MeetingNoteModel {
    MeetingNoteModel {
        id: ss(m.id.clone()),
        title: ss(m.title.clone()),
        body: ss(m.body.clone()),
        started_label: ss(stamp_utc(m.started_at)),
    }
}

/// Docks the floating window to the right edge of the primary screen.
fn place_at_right_edge(win: &FloatingNotesWindow) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN};
        // SAFETY: GetSystemMetrics takes no pointers and has no preconditions.
        let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let size = win.window().size();
        if screen_w > 0 {
            let x = (screen_w - size.width as i32 - 12).max(0);
            win.window().set_position(slint::PhysicalPosition::new(x, 80));
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = win;
    }
}
