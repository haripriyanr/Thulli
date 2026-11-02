# AGENTS.md — Agent & Developer Guide for Thulli

Welcome to **Thulli**! This document provides authoritative architectural guidelines, constraints, design principles, and operational procedures for all AI coding agents and human contributors working on this repository.

---

## 1. Project Mission & Identity

- **Application Name**: Thulli
- **Repository**: `E:\repos\Thuli`
- **Core Purpose**: A 100% native Rust productivity app (Pomodoro, Spaces, Notes, Settings, AI planning) with local encrypted storage and reusable Slint UI components. See §13 for the current state.
- **Tech Stack**:
  - **Language**: 100% pure Rust (Slint UI 1.18 + Tokio).
  - **Crate Structure**: Dual-target package providing `thulli_core` library crate (`src/lib.rs`) and `thulli` desktop application binary (`src/main.rs`).
  - **Storage**: Local-first, authenticated AES-256-GCM encryption at rest derived via Argon2id.
  - **OS Target**: Windows (with native Win32 window management) and cross-platform desktop compatibility.

---

## 2. Non-Negotiable Core Directives

### 2.1. Absolute Keyword Ban (Zero Tolerance)
> [!CAUTION]
> **STRICTLY NO REFERENCE TO `blitz` OR `locu`** in any part of the codebase:
> - No occurrences in source code, variable names, struct/enum names, or function signatures.
> - No occurrences in comments, docstrings, or markdown documentation.
> - No occurrences in UI labels, placeholders, titles, or debug logs.
> - Always run the keyword audit script before finalizing any changes.

### 2.2. Pure Rust Only
- The entire desktop application must remain **100% Rust** using the Slint GUI toolkit and Tokio asynchronous runtime.
- Do NOT introduce web wrappers, Electron, Tauri, or JavaScript runtimes.

### 2.3. Consumer-Grade UI/UX (Zero Developer Technobabble)
- Never expose raw cryptographic jargon or internal implementation details to the user (e.g. avoid *"Encrypted AES-256 Vault"*, *"Database Serializer"*).
- Present clean, human-centered language: *"Focus Queue"*, *"Your Lists"*, *"All my lists"*, *"Search tasks"*, *"Active Protection"*.
- Do NOT hardcode personal names or greetings (e.g., avoid *"Good Morning, Hari"*). Keep UI welcoming, generic, or dynamic.

### 2.4. Iconography Standards
- **Strict Rule**: NO flat solid color block icons, NO emojis (like `⚡`, `📋`, `✕`), and NO solid colored buttons.
- **Accepted Standard**: Pure filled-in duotone vector icons with 15–20% translucent gradient fills and crisp geometric strokes.
- Use only the components defined in [`ui/icons.slint`](file:///e:/repos/Thuli/ui/icons.slint) (`IconBolt`, `IconFolder`, `IconArchive`, `IconShield`, `IconScratchpad`, `IconSettings`, `IconSun`, `IconCalendar`, `IconInbox`, `IconRocket`, `IconPlus`, `IconClose`, `IconTrash`, `IconSearch`, `IconClock`, `IconPlay`, `IconPause`, `IconSkip`, `IconCheck`, `IconArrowRight`, `IconChart`, `IconMore`, `IconChat`, `IconUser`, `IconChevronLeft`, `IconChevronRight`, `IconNote`, `IconStar`, `IconFlag`). Add new icons there in the same duotone style.

---

## 3. Technology Evaluation: Slint vs. Dioxus

When building desktop productivity applications, developers frequently compare Slint with Dioxus:

| Dimension | Slint (Chosen Architecture) | Dioxus (Desktop / Wry) |
| :--- | :--- | :--- |
| **Rendering Engine** | 100% GPU-accelerated native renderer (FemtoVG / Skia) compiling to machine code | System WebView2 (HTML/CSS browser container) |
| **RAM Footprint** | Ultra-lean **15–35 MB RAM** | **75–150+ MB RAM** (due to WebView runtime) |
| **Startup Latency** | Sub-50ms instant execution | 150–400ms WebView2 initialization |
| **Frameless / HUD Mode** | Trivially easy native Win32 window clipping, alpha transparency, and always-on-top | Requires WebView transparency hooks, prone to edge artifacts |
| **Web Cloning Speed** | Slower initial UI development (requires building custom components) | Faster initial UI cloning (supports Tailwind CSS classes directly) |

**Conclusion**: While Dioxus allows faster initial 1:1 CSS copying, **Slint was chosen for Thulli to deliver an uncompromising, featherweight, true native desktop utility** that runs continuously in the background with negligible CPU and RAM overhead.

---

## 4. Object-Oriented Programming (OOP) & Architecture Standards

All code in Thulli must follow clean Object-Oriented and SOLID design principles, emphasizing encapsulation, dependency inversion, and separation of concerns:

### 4.1. Domain Models (`src/models/`)
- Split by area: `task.rs`, `list.rs`, `notes.rs`, `focus.rs`, `shield.rs`, `chat.rs`, `tag.rs`, `preferences.rs`; `mod.rs` holds the root aggregate `AppStateStore` and re-exports everything.
- Entities are encapsulated structs with rich behaviour, factory constructors (`Task::new()`, `TaskList::new_personal()`, `BlockedApp::new()`) and state transitions (`task.complete()`, `task.move_to_section()`, `list.toggle_archived()`).
- Every persisted field uses `#[serde(default)]` so older files still load. `AppStateStore::migrate()` upgrades old data; bump `CURRENT_SCHEMA_VERSION` when a migration is added, and extend the migration test.
- Task sections are `backlog`, `this_week`, `today`; Done is derived from `TaskStatus::Completed`.

### 4.2. Repository Pattern (`src/repository/mod.rs`)
- Persistence is abstracted behind the `StorageRepository` trait (`load_with_report`, `load`, `save`).
- `EncryptedStorageRepository` provides local-first AES-256-GCM encrypted persistence. Writes are atomic (temp file, then rename). A file that cannot be decrypted or parsed is kept as a timestamped `.bak` copy before starting fresh, and the failure is returned as a `RecoveryNotice` that the shell shows to the user.

### 4.3. Domain Services (`src/services/`)
- One file per area: `tasks.rs`, `lists.rs`, `timer.rs`, `today.rs`, `notes.rs`, `insights.rs`, `preferences.rs`, `shield.rs`, `assistant.rs`, `history.rs` (undo and redo seam), plus `store_handle.rs`.
- Services read and write state only through `StoreHandle`, which saves after every `mutate`.
- `TimerService` is driven by `tick()` from a `slint::Timer` in the shell; every transition also has an `_at(Instant)` variant for tests.

### 4.4. Platform Abstraction (`src/window/mod.rs`)
- Low-level Win32 unsafe calls are encapsulated behind the safe, modular `WindowManager` interface (`begin_native_drag()`, `minimize()`, `toggle_maximize()`, `exit_cleanly()`).

### 4.5. UI Wiring
- Screens (Pomodoro, Spaces, Notes, Settings, plus the AI Plan panel and Sort screen) are live and show only saved data. Every value they display comes from `AppData` (`ui/app_data.slint`), which Rust fills from the saved state in `src/data_hub.rs`; display rows are built in `src/convert.rs`.
- Screens never hold sample or invented data. A screen with nothing saved shows an empty-state line.
- `DataHub` changes data only through the services (`TaskService`, `NotesService`, `PreferencesService`, `StoreHandle`) so every change is saved. Do not write to the store from a screen.

---

## 5. Reusable UI Library Component Standard (`ui/components/`)

The user interface must never be implemented as a monolithic file. Screens live in `ui/screens/`; reusable widgets live in `ui/components/` with configurable public properties. Build new UI from these components.

```
ui/
├── app.slint                 # Screen-free shell: title bar, theme control, background
├── shell_state.slint         # ShellState global: window controls and ambient motion
├── theme.slint               # Theme palette, spacing, border radii
├── types.slint               # Shared structs for every screen
├── icons.slint               # Duotone vector icon library
├── window_controls.slint     # Frameless window action buttons
└── components/
    ├── navigation_rail.slint  # Responsive left rail and compact touch navigation
    ├── buttons.slint  inputs.slint  cards.slint  topbar.slint  sidebar.slint  modals.slint
    ├── tabs.slint            # ThulliTabPill (Today, Tasks, Insights)
    ├── timer_pill.slint      # ThulliTimerPill (global running timer)
    ├── app_menu.slint        # ThulliAppMenu
    └── empty_state.slint     # ThulliEmptyState
```

### 5.1. Component Design Guidelines
1. **Explicit Touch Hit-Testing**: Interactive components must define their outer bounds on `TouchArea` with explicit `width` and `height`, eliminating Slint's `0×0` hit-testing dead zones.
2. **Prop Aliasing & Two-Way Binding**: Inputs and stateful controls expose two-way bound properties (`in-out property <string> text <=> ...`).
3. **Event Callbacks**: Components communicate state changes upward via callbacks (`callback submitted(string)`, `callback clicked()`, `callback toggled()`).

---

## 6. Directory Layout & Module Responsibilities

```
E:\repos\Thuli\
├── Cargo.toml                # thulli_core library + thulli application binary
├── build.rs                  # Slint compilation build script
├── docs/PRODUCT_SPEC.md      # Target behaviour
├── src/
│   ├── lib.rs                # Library root exporting reusable modules
│   ├── main.rs               # Entry point: load saved theme, create shell, manage native window
│   ├── parser.rs             # Natural language task parser (single file)
│   ├── security.rs           # SecurityManager: Argon2id + AES-256-GCM (single file)
│   ├── models/               # task, list, notes, focus, shield, chat, tag, preferences + mod.rs aggregate
│   ├── repository/mod.rs     # StorageRepository trait, encrypted atomic storage, backup on failure
│   ├── services/             # One file per area, StoreHandle, timer, undo history
│   ├── window/mod.rs         # WindowManager (Win32 frameless window management)
└── ui/                       # Screen-free shell and reusable component library (see section 5)
```

## 7. Natural Language Task Parser

Located in the single file [`src/parser.rs`](file:///e:/repos/Thuli/src/parser.rs), the smart parser extracts actionable metadata from single-line task descriptions:

- **Duration Patterns**:
  - `(\d+)\s*h(?:ours?)?\s*(\d+)\s*m(?:in(?:ute)?s?)?` (e.g. `1h 30m` -> 90 mins)
  - `(\d+)\s*h(?:ours?)?` (e.g. `2h` -> 120 mins)
  - `(\d+)\s*m(?:in(?:ute)?s?)?` (e.g. `45m` -> 45 mins)
- **Tag Patterns**:
  - `@today` -> board `"today"`
  - `@week` -> board `"week"`
  - `@backlog` -> board `"backlog"`

---

## 8. Verification & Quality Standards

Before committing any changes, agents must execute and pass the following quality verification gates:

### 8.1. Keyword Audit Script
Ensure zero occurrences of prohibited words across all source and UI files:
```powershell
python -c "
import os
forbidden = ['blitz', 'locu']
matches = []
for folder in ['src', 'ui']:
    for root, dirs, files in os.walk(folder):
        for f in files:
            path = os.path.join(root, f)
            with open(path, 'r', encoding='utf-8', errors='ignore') as fp:
                for idx, line in enumerate(fp, 1):
                    for kw in forbidden:
                        if kw in line.lower():
                            matches.append(f'{path}:{idx}: {line.strip()}')
if matches:
    print('Found forbidden keywords:')
    for m in matches: print(m)
    exit(1)
else:
    print('Clean! ZERO forbidden keywords found.')
"
```

### 8.2. Automated Test Suite
All unit, integration, and round-trip encryption tests must pass cleanly:
```powershell
cargo test
```

### 8.3. Compilation Check
No compile errors or compiler warnings:
```powershell
cargo check
cargo build --release
```

---

## 9. Motion Policy

- The shared `AppBackground` (ui/components/ambient_background.slint) is the one backdrop for every screen. It is a gradient with three soft orbs that drift slowly on a shared `background-phase` driven by the shell timer. Screens stay transparent and sit on it.
- Glass surfaces (`GlassCard`, `GlassPill`, `GlassIconButton` in ui/components/glass.slint) are translucent fills with a hairline edge. Use them for panels and controls instead of solid theme fills.
- Keep the interruptible 260ms native maximize/restore morph as the only shell transition. Apply each Windows frame's position and size in one native update.
- For future screen-level transitions, animate only when motion clarifies feedback or spatial change. Respect reduced-motion preferences where supported.

## 10. Telemetry & Crash Logging (dev phase)

Dev builds ship lightweight local telemetry to make performance work and crash triage possible:

- **Location**: `src/telemetry.rs` (binary-crate module, wired from `main.rs`). All output goes to `logs/` relative to the current working directory — created lazily, never committed (`.gitignore` covers `logs/`).
- **Sampling**: `Sampler::log()` appends one line to `logs/telemetry.log` every 5 seconds: timestamp, FPS, process CPU%, process RAM, system RAM%, and GPU utilisation.
- **GPU counter**: read via PDH `\GPU Engine(*)\Utilization Percentage` (English counter path), filtered to this process's instances. If PDH setup or sampling fails for any reason, the line falls back to `gpu=n/a` — never panic, never block the UI thread.
- **Crash hook**: `install_crash_hook()` captures panics with a backtrace, writes `logs/crash-<timestamp>.log`, marks the event in `telemetry.log`, then chains the original hook so default panic behavior is preserved.
- **Off switch**: set `THULLI_TELEMETRY=0` to disable logging (crash files are still written). Telemetry is ON by default in dev.
- **Rules**: telemetry code must never panic, never block the UI, and never surface technical jargon in the UI. File/FFI failures are silent by design.

## 11. Product Direction (Proposed Features)

Three features that would make Flow stand out:

1. **Immersive Focus Mode**
   Hide the entire dashboard. Show only the timer, task name, progress ring and ambient background.

2. **Flow Intelligence**
   Analyze when you're most productive, your average focus duration and task completion trends.

3. **Task-Aware Pomodoro**
   Select a task, estimate the required sessions, track time spent and automatically update task progress.

Recommendation: make Flow more than a Pomodoro timer. Make it a complete productivity system with Focus, Tasks, Analytics and Timesheets working together.

## 12. Spaces Behaviour Spec

Behaviour rules observed in a third-party reference app (read-only study of its installed bundle and UI strings). Written in plain English. "Space" is Thulli's term for a list. Items marked (unverified) were not confirmed by code or UI.

### a. Spaces (lists)
- Each task belongs to exactly one space (unverified: a default space is created for new users).
- A space has a name and a colour and an optional icon or letter badge (verified).
- Tasks without a space are shown under the label "Unsorted" (verified).
- An "All" combined view shows tasks from all visible spaces at once (unverified).
- Per-space counts include only open tasks, not done or archived ones (verified for the queue count).
- Deleting a space: tasks in it are moved or removed (unverified; check before implementing).
- Archiving is a separate action from deleting (verified label exists, behaviour unverified).

### b. Board columns
- Four columns: Backlog, This Week, Today, Done (verified).
- Progress reads as completed over total. Done tasks and archived tasks are excluded from the open count (verified).
- Column collapse and expand behaviour: (unverified).
- Done column groups finished tasks under their completion date, one group per local calendar day (verified).
- Adding a task to the Done column creates it in Backlog instead (verified).
- Back and forward column moves at the ends of the sequence: (unverified).

### c. Task card
- Ticking a task marks it done and moves it to the Done column (verified).
- Unticking a done task returns it to Today at the position it held before, appended as the first item of Today (verified).
- Subtasks: each subtask has its own done flag. Ticking one subtask only changes that subtask (verified in the editor state).
- Ticking the last subtask does not complete the parent task in the editor (verified in the client; server behaviour unverified).
- The card shows a count of completed subtasks over total subtasks (verified).
- Estimate is stored in minutes and shown on the card; an estimate can be entered when creating a task (verified).
- Deadline urgency is derived from the time left until the deadline compared with the estimate: "urgent" when the deadline has passed or less time remains than the estimate; "soon" when under 24 hours remain or under twice the estimate remains; otherwise "comfortable" (verified).
- Tags, notes and "+ EST" shortcut: (unverified).
- Do date and deadline are separate fields in the scheduling editor (verified labels); exact effect of each on column placement: (unverified).
- Edit mode Save and Escape behaviour: (unverified).
- The more menu contents: (unverified).

### d. Launch
- Launch is a rocket action on the card (verified).
- Launch moves the task to the top of the Today queue. It does nothing for tasks that are done, archived, recurring templates, or not standard tasks (verified).
- If the task is already first in Today, Launch changes nothing (verified).
- Whether Launch starts a timer or a fixed duration: (unverified).

### e. Matrix
- Four quadrants with these display labels: "Do ASAP", "Later today", "Mid week", "Leave till end" (verified). Internal keys are do, schedule, delegate and drop (verified).
- Quadrant assignment can be requested from an AI prioritisation service in batches of up to 200 tasks (verified). Thulli should treat this as optional and never required for the matrix to work.
- Quadrant to column mapping: (unverified).
- Unsorted queue and "review one by one" flow: a review screen exists (verified string), exact steps: (unverified).
- Moving tasks between quadrants and columns: (unverified).

### f. Calendar
- Tasks can be placed on the calendar by a start time and an end time. Time blocks are separate items that are hatched in the grid (verified).
- Default selection length is 60 minutes (verified constant). Default end offset is 30 minutes (verified constant, use unverified).
- Past items are visually dimmed by a shared style class (verified). Whether past days are hatched: (unverified).
- Today is highlighted (unverified).
- Unscheduled tasks appear in a rail grouped by Today, This Week and Backlog (verified).
- Dropping a task onto a date sets its start date. If the date is today it goes to Today, if it falls in the current week it goes to This Week, otherwise Backlog (partly verified; the later-week case is unverified).
- Day, week and month views: (unverified).

### g. Undo and redo
- Undo and redo are available by Ctrl+Z and Ctrl+Shift+Z (verified).
- Shortcuts are ignored while focus is in a text input, text area, select, or editable element (verified).
- The toolbar buttons show the label of the next action to undo or redo, or "Nothing to undo" / "Nothing to redo" (verified).
- The undo control is behind a build-time feature flag (verified; flag value unverified).
- Undo scope (which actions are recorded): (unverified).

## 13. Current State and Handoff (2026-11-03)

### a. Where things live
- **Pomodoro** `ui/screens/pomodoro_screen.slint`: ring with elapsed gradient, full screen toggle (ring expands, only toggle and play/pause shown), task picker dropdown, ambient scene tiles, analytics with Recent sessions. Controls in `ui/components/focus_controls.slint` (PlayButton, SceneTile, FieldCard, SearchField, StatCard, IconTab).
- **Spaces** (replaced Tasks) `ui/screens/spaces_screen.slint`: Board (collapsible columns, back/forward), Matrix (four quadrants + Unsorted, "Sort one by one"), Calendar (week, unscheduled rail). Spaces start empty; no default space is ever created. Rust side: `src/spaces.rs` builds rows, `src/data_hub.rs` `wire_spaces` handles callbacks. Widgets in `ui/components/space_widgets.slint` (TaskCard, ColumnShell, TaskComposer, SpaceForm, DateField, CardAction, MicButton).
- **TaskCard**: hover actions float over the top-right corner (no layout change on hover). Add subtask lives in the edit mode only. Subtasks are tasks; deleting one calls `space-task-deleted`.
- **Drag and drop** `ui/components/drag.slint`: `DragState` global, `DropZone` overlays, `DragGhost`. `TipState` + `TipLayer` draw every hover tooltip at window level (never inside a container).
- **Settings** `ui/screens/settings_screen.slint`: Language (English only) at top, AI (key, model, Test), Timer settings. Controls in `ui/components/settings_controls.slint`.
- **AI** `src/services/assistant/` (engine trait, `RemoteEngine` for OpenRouter, strict JSON action parser, short 6-char task ids, compact prompts in `prompt.rs`) and `src/data_hub_ai.rs` (worker thread, approve-before-apply). UI in `ui/screens/ai_panels.slint` (PlanPanel, SortScreen). Prompts are kept small on purpose: a bundled local model (Qwen3.5 0.8B, fine-tuned later) will replace the remote one behind the same `ChatEngine` trait.
- **API key**: Windows Credential Manager via `src/secrets.rs` (`Thulli/openrouter-api-key`). Never store it in the data file.
- **Voice**: `src/voice.rs` sends Win+H (Windows voice typing). An ASR model will replace `toggle_voice_typing` later; UI already calls it through `AppData.mic-toggle`.
- **Window**: frameless with native resize (`begin_native_resize` in `src/main.rs`; Slint gets PointerReleased/Exited before SC_SIZE so input is not stuck).

### b. Open issues (not yet verified in the running app)
- Drag and hover glitch: fixed in code, not yet confirmed in the app. Cause: the Spaces Flickables were interactive, so they took over pointer drags and sent the card a cancel. All Spaces Flickables now have `interactive: false` (the wheel still scrolls); keep it that way for any new scroll area that holds cards. The 1 s Pomodoro tick only sets timer values, never card models, so it was not the cause. TaskCard drag logic is in `press()`/`drag-to()` (window coordinates); subtask rows forward their pointer events to it, so a drag can start from a subtask.
- TaskCard changes (floating hover bar, subtask delete, two-row DateField, add subtask in editor) compile but were not checked visually.
- Pomodoro task picker "No due date" rows fix not confirmed visually.

### c. Pending work
- ASR model for voice typing (user will bring it).
- Fine-tuning Qwen3.5 0.8B on Kaggle for Thulli actions in all its languages, low reasoning tokens. A side session on branch `claude/sleepy-diffie-8d9c7b` stalled because the Kaggle connector was unauthenticated; nothing from it is merged.

### d. Working agreements with the user
- Work screen by screen; the user sends reference screenshots. Match them but never copy another product's name or branding (§2.1).
- Do not hardcode spaces, names or sample data.
- Commit only when asked; run the keyword audit (§8.1) and `cargo check` first.
- The user often says "no need to test"; still say plainly what was not verified.
- Write Slint edits with a script or Edit tool; avoid PowerShell `Set-Content` (it mangles `·` and adds BOMs).
