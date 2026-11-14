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
- **Settings** `ui/screens/settings_screen.slint`: Language (115 choices) at top, AI (models: Download now, Refresh, Remove, Ready badge), Timer settings. Controls in `ui/components/settings_controls.slint`.
- **AI** runs 100% on device through ONNX Runtime (`ort` crate; no OpenRouter, no API key). `src/ai/`: `pack.rs` downloads one zip (`konradjr007/thulli-models` on Hugging Face, `thulli-models.zip`) on first use into `%LOCALAPPDATA%\thulli\models`, checks its SHA-256 from the HF API, unpacks `llm/`, `asr/`, `tts/`, deletes the zip, resumes interrupted downloads. `llm.rs` is the planner: Qwen3.5-2B Polaris (thinking ON; it thinks less when not needed) as 4-bit ONNX, decoder loop carrying conv/recurrent/KV state, sampling 0.6/top-k 20/top-p 0.95, `strip_thinking` before parsing. `asr.rs` is Omnilingual ASR CTC 300M int8 (raw 16 kHz in, greedy CTC). `tts.rs` ports OmniVoice's 32-step unmasking (int4 backbone + fp16 Higgs decoder, 24 kHz). `Models` loads each once. `src/services/assistant/` keeps the `ChatEngine` trait, strict JSON action parser, short 6-char task ids and compact prompts; prompts ask for every title/step/note in the app language. `src/data_hub_ai.rs` runs models on worker threads (approve-before-apply). UI in `ui/screens/ai_panels.slint` (PlanPanel with Read aloud, SortScreen); Settings > AI has Download now / Remove. `examples/ai_smoke.rs` downloads the pack and runs all three models once.
- **AI actions** (all approve-before-apply, parsed in `actions.rs`, applied in `assistant/mod.rs` `apply_one`): create_task (with steps, quadrant, do date; a new space name creates the space), update_task, complete_task, delete_task, add_subtask, set_quadrant, schedule_task, create_note, create_space, rename_space, delete_space, start_focus (links the timer to the task) and stop_focus (logs the time to the task). Subtasks are tasks, addressed by their own short id; the prompt lists them under their parent. Oversized estimates are capped (480 min) and extra steps dropped (20), never the whole action. When nothing is usable the Plan panel shows the first rejection reason. Reply cap is 2500 tokens.
- **Languages**: 115 choices in `LANGUAGE_CHOICES` (`src/services/preferences.rs`), shown A-Z in `ui/components/language_picker.slint`. UI text is `@tr()`; drafts for all 115 are in `lang/<id>/LC_MESSAGES/thulli.po` (machine-drafted, need native review, weakest: cjk, fuv, wo, bo, bug, bjn, ks, sd, mni, tcy, regional Arabic). build.rs bundles them with `DefaultTranslationContext::None` (the .po files have no msgctxt) and reruns when `lang/` changes. `slint::select_bundled_translation` runs at startup and on change. Not done: Noto fonts for all scripts, right-to-left mirroring.
- **Drag look**: `DragGhost` is a full-size, slightly tilted copy of the card held at the grab point; the source card turns invisible while carried.
- **Polaris conversion**: no ONNX build of Polaris existed. A Kaggle script (notebook `notharipriyan/thulli-polaris-onnx`) transplants Polaris weights into onnx-community/Qwen3.5-2B-ONNX's graph by matching each initializer to the base Qwen tensor by value (as-is, transposed, +1 norm offset, squeezed conv1d), stops if anything is unmatched, then quantises with MatMulNBits (block 32). Run 3: 272 decoder weights + embeddings replaced, 0 unmatched. Kaggle CLI auth: pass `KAGGLE_API_TOKEN=$KAGGLE_TOKEN`.
- **Voice**: `src/voice.rs` records the default mic with `cpal` (mono, resampled to 16 kHz) and plays speech with `rodio`. Each `MicButton` has an `owner`; `AppData.mic-toggle(owner)` starts/stops, the transcript comes back via `mic-owner`/`mic-text`/`mic-seq` and only that field's `heard` fires.
- **Window**: frameless with native resize (`begin_native_resize` in `src/main.rs`; Slint gets PointerReleased/Exited before SC_SIZE so input is not stuck).

- **Goals** (`ui/screens/goals_screen.slint`, `ui/components/goal_widgets.slint`, `src/data_hub_goals.rs`, `src/services/goals.rs`, `src/models/goal.rs`): GOALS → MILESTONES → TASKS → FOCUS. A goal has a status (active, paused, completed, archived), a priority, an optional target date and one of two progress modes: `Milestones` (done over total; no milestones means no percentage, never 0 or NaN) or `Measured` (current over target with a unit, clamped to 0-100 but real values kept; a target of zero or less is rejected). Tasks are never copied: `Task::goal_id` and `Task::milestone_id` link an existing task, so a task belongs to at most one goal (linking it to another goal moves it) and duplicates cannot exist. Deleting a goal or milestone only clears those fields; tasks survive. Finishing tasks never completes a milestone by itself. Focus time comes from `insights::collect_spans` (Focus kind, attributed through the top-level task of a step), so a session is counted once; `goals::focus_summary` reads the span list once for the dashboard. Schema version 5 adds `goals` and the two task fields, all `#[serde(default)]`. Navigation: rail item "Goals" (`goals`) sits between Spaces and Pomodoro. Weekly review (`goals::weekly_review`, "Weekly review" button on the dashboard): last seven days ending today, built only from saved records: milestones and linked tasks completed, focus time on goals, goals that moved, deadlines and milestone dates in the next seven days, overdue or quiet (14 days) goals, and up to three suggested priorities. AI: `create_goal` (title, description, priority, target_date, milestones, tasks pinned by milestone title) and `link_task` are normal approve-before-apply actions; a duplicate goal title is refused. The plan prompt lists active goals and calendar anchors (`+3 months 2027-02-08`) because the 2B model cannot add months. Reply parsing has repair steps for what the small model really writes (escaped quotes, a missing comma or closing bracket, junk after the block, one action in the root object, stray task keys on a goal); see `actions.rs` `repair_envelope` and `salvage_envelope`. In 6 real runs of "complete my VLSI research project in three months" 5 gave a usable goal; one ran to the 3072 token cap. The Pomodoro task picker shows the goal name in each task's meta line. Telemetry: `telemetry::log_timing(category, name, start)` (developer builds only), a background writer thread and a 4 MB rotation to `telemetry.log.1`. Text: Rust never builds English sentences for Goals. It sends facts (`due-kind`, `due-n`, counts, values) in `GoalRow`, `MilestoneRow`, `GoalTaskRow` and `ReviewRow`, and the `GoalText` global in `goal_widgets.slint` phrases them with `@tr()` so every sentence is translatable (dates still print as "9 Nov", and a goal's saved history log is stored in English). All 143 Goals/Settings strings are translated for the 115 languages (machine drafts from Gemini, validated for placeholders and untouched old entries; need native review). Review tool: `examples/ai_eval.rs` runs the planner on a JSONL of requests per language and domain. Idle cost: the background drift (`BACKGROUND_FRAME` 50 ms, env `THULLI_BG_MS` to experiment, 0 stops it) costs about 9% of a core and 8% GPU in release; 100 ms halves it (see the A/B in the handoff report). The Today screen's pill is the action "Later" (moves a task to This Week), not a section label. Gemini's UI patch (theme contrast, calmer orbs, bounds and pressed states, GlassPill auto width, Pomodoro breathing from `background-phase`) is applied; its `max-width: root.width * 0.46` on review rows made a binding loop and was removed. Not done yet: goals in Insights, Chat, a native-speaker review of translations.
- **QA tooling** (not shipped): `examples/qa_fixture.rs` writes an encrypted synthetic data file (`empty|normal|heavy|stress`) into a folder you give it; start `thulli.exe` with `APPDATA` pointing at that folder so real data is never touched. Never type into the app with synthetic keystrokes unless the app is verified to be the foreground window right before sending.

### b. Open issues (not yet verified in the running app)
- Drag and hover glitch: fixed in code, not yet confirmed in the app. Cause: the Spaces Flickables were interactive, so they took over pointer drags and sent the card a cancel. All Spaces Flickables now have `interactive: false` (the wheel still scrolls); keep it that way for any new scroll area that holds cards. The 1 s Pomodoro tick only sets timer values, never card models, so it was not the cause. TaskCard drag logic is in `press()`/`drag-to()` (window coordinates); subtask rows forward their pointer events to it, so a drag can start from a subtask.
- TaskCard changes (floating hover bar, subtask delete, two-row DateField, add subtask in editor) compile but were not checked visually.
- Pomodoro task picker "No due date" rows fix not confirmed visually.

### c. Pending work
- Verify in the running app: mic transcription, Read aloud, planner speed on CPU, translated UI with non-Latin scripts.
- Fine-tune the planner (Polaris) for Thulli actions in all languages with fewer reasoning tokens (now done with `tools/finetune/`, see c2). The earlier attempt in `tools/assistant_ft/` is kept on main for reference only; it is superseded. The repo has one branch, `main`.

### c2. State, goals and TODO (handoff 2026-11-14)

**Goal of the planner work.** The on-device model (Qwen3.5-2B Polaris, LoRA fine-tuned) gives correct tool calls in all 115 languages (aim: at least 95% on every language, measured, not promised), uses few tokens, never chats outside the Chat screen, and web research works with sources. Architecture rule: the model proposes, `actions.rs` validates, the app executes only after the user presses Apply; saved state is the truth.

**What exists** (committed 2026-11-14, see git log): Goals; Chat (threads, edit in place with older replies, copy, fork, delete, day headers, Stop, scroll button, voice ring, border beam, thinking orb); AI kit `ui/components/ai_kit.slint`; hover tips everywhere (`ui/components/tips.slint`); window fixes; own search layer `src/search/` and deep research `src/research/` (no AGPL); fine-tune tooling `tools/finetune/` (scratch `tools/finetune/work/` is gitignored and holds every dataset, zip and result).

**Fine-tune results.** Strict pass on held-out requests:
- Round 1 (`notharipriyan/thulli-ft-r1`): 93.5% on 2,070 templated tests.
- Round 2 (`thulli-ft-r2`): 92.4% on 4,140 harder templated tests (plan 80% -> 98%).
- Round 3 (`thulli-ft-r3`, small files in `work/r3out/`, re-scored with `work/rescore.py`): templated 95.0%, real MASSIVE requests 94.3%, research steps 88.4% (answers 68%: loops and dropped facts in weak languages), native-written 55.8%. The native score is five test items failing in every language (behaviour, not language): "day after tomorrow"/"in 3 days" counted wrong; "to do the day after tomorrow" set as deadline instead of do_date; "yesterday I spent 50 min on <new task>" without the date; small talk turned into tasks; a note's body dropped.
- Round 4 (`thulli-ft-r4`) STARTED 2026-11-14 about 21:10 on 2x T4 from r3's adapter, expect about 6 h. Fixes for the above: the planner prompt (`prompt.rs`) now has a `Days:` line (two days back to a week ahead with weekdays; test `day_list_crosses_month_ends`), names the busiest part of the day in the focus line, and states do_date vs deadline, past dates, note bodies and small talk. New scenarios: `skeletons.py` `gaps` (51; translated by Gemini, merged in `work/translations_r4.json`; scenarios in `work/skeletons_r4.json`, the old 192 kept as saved because round 2 edited plans in place, so never regenerate old ones). Data: `assemble_round3.py` with `SKELETONS=skeletons_r4.json TRANSLATIONS=translations_r4.json` -> `work/r4/`; `work/gaps_suite.py` makes the new held-out rows a `gaps` suite tested in full; 17,614 train rows, 7,455 test rows (templated 3 per cell, gaps 1,725, native 1,596, research 747, MASSIVE 282). Read it with `kaggle kernels output notharipriyan/thulli-ft-r4 -p work/r4out --file-pattern "(summary|results|log)" -q`, then `py -I work/rescore.py work/r4out/results.jsonl work/r4/rows.jsonl` and `py -I work/nat_items.py` (point it at r4out) for the per-item native view.

**Scorer** (`kaggle_train.py`): strict for actions; research steps checked for shape and invented numbers; research answers must keep half the reference numbers, invent none and not loop (one 4-word run 6+ times); MASSIVE titles count when one holds the other. `work/score_selftest.py` must report 0 gold failures before any round.

**Kaggle how-to.** CLI `C:\Users\harip\AppData\Roaming\uv\tools\kaggle\Scripts\kaggle.exe` with `KAGGLE_API_TOKEN=$KAGGLE_TOKEN` (the Kaggle MCP is not logged in). Job: `tools/finetune/mk_kernel.py <folder> <slug> KEY=VALUE...` (TRAIN_PER_LANG=0 means all training rows, EVAL_PER_CELL limits only the templated suite, RESUME=auto with `KERNEL_SOURCES=notharipriyan/thulli-ft-rN`), push with `kernels push -p <folder> --accelerator NvidiaTeslaT4`. Dataset `notharipriyan/thulli-finetune-data`: upload `rows.jsonl.gz` (the 100+ MB plain file uploads too slowly; Kaggle unpacks it; `kaggle_train.py` reads either). Fetch only small output files; never pull the adapter or merged model.

**Data rules.** Never download raw datasets to the PC; process them in a Kaggle job (`pool_data.py` made `work/pool_out/pools.json` and `massive_rows.jsonl` from MASSIVE 1.1, CC BY 4.0, and Tatoeba, CC BY 2.0 FR; attribution must go in the README before release). Flores/SIB (CC BY-SA) not used. Gemini returns: research translations merged in `work/research_clean.json` (12,978 of 13,110 kept by `research_merge.py`), native dataset in `work/nat_new/` (4,788 examples, used 4 train + 2 test per domain per language), round 4 scenarios merged, UI strings merged. Old zips were deleted at the user's request.

**Chat findings (2026-11-14).**
- The Chat screen uses a different prompt (`prompt.rs` `build_prompt` / `system_prompt` with `FORMAT_HELP`) from the Plan prompt the model is trained on (`plan_prompt` with `PLAN_RULES`). No round has trained the Chat prompt, so Chat answers come out loose (for example bare `{"type":"delete_task",...}` lines). Round 5 must also render rows with the Chat prompt.
- Fixed in the app: `parse_reply` reads loose action objects as proposals (`loose_actions`), never shows JSON in a bubble (`hide_json`; "[1]" source marks are kept), and when a reply tried to change something but nothing usable came of it, Chat shows "No changes were made." (`TurnOutcome.nothing_changed`, phase "no-changes"). Nothing is ever applied without Apply.
- Chat phases: `AppData.chat-phase` is loading / reading (prompt being read, until the first token) / writing / searching / stopped / no-changes; the screen phrases them with `@tr`. Stop acts at once: each run has a number and its own stop switch (`DataHub::new_run`); a stopped run's late answer is dropped.
- Border beam: rebuilt (fixed-length beam with a fading tail measured along the border, frame-clock driven, glow layer). It moves now; the user wants more polish later.

**Waiting on the user.** `work/thulli-ui-translation.zip` (5 strings: Loading the AI..., Reading your message..., Searching the web..., Stopped., No changes were made.) for Gemini; check the answer with `tools/finetune/ui_check_all.py`, merge with `merge_ui.py`.

**TODO (tick when done)**
Fine-tune and model
- [ ] Read round 4: did the gaps suite and the five native items improve; templated stays at or above 95%; research answers above 68%.
- [ ] Round 5: also train the Chat prompt (render the same specs through `build_prompt` with a thread holding the request), more research answers for weak languages, keep the native set held out.
- [ ] Convert the merged model to the 4-bit ONNX pack (notebook `thulli-polaris-onnx`, point it at the merged weights), upload to the models repo, re-run `examples/ai_eval.rs` in the app; set `MAX_NEW_TOKENS` (now 1200) from the longest correct reply.
- [ ] Per-language support tiers in Settings from measured results (supported 95%+ on templated, native and real tests; beta 88-95; experimental below). Do not claim support the tests do not show.
Product
- [ ] Beam polish (user asked to come back to it).
- [ ] Today rows: put "Later" and the delete icon at the right edge (they move with the title length).
- [ ] Screen checks not done: voice ring, other window sizes (automation cannot reach the title bar; use a real mouse there).
- [ ] `ProposedAction::describe()` lines (approve cards) are English only: build them from facts and phrase them with `@tr`.
- [ ] Research quality: check [n] marks against notes, PDFs, a real Deep (100+ pages) run, Google or Brave only with a browser-like connection.
- [ ] RAG for Chat across chats and tasks: EmbeddingGemma-300M ONNX (256 dims, prefixes `task: search result | query: ` and `title: none | text: `, Gemma licence notice), after the fine-tune.
- [ ] Not started: goals in Insights, quick-capture hotkey, shield wiring, native-speaker review of translations.
- [ ] README: credit MASSIVE (CC BY 4.0) and Tatoeba (CC BY 2.0 FR) before release.

### d. Working agreements with the user
- Work screen by screen; the user sends reference screenshots. Match them but never copy another product's name or branding (§2.1).
- Do not hardcode spaces, names or sample data.
- Commit only when asked; run the keyword audit (§8.1) and `cargo check` first.
- The user often says "no need to test"; still say plainly what was not verified.
- Write Slint edits with a script or Edit tool; avoid PowerShell `Set-Content` (it mangles `·` and adds BOMs).
- Never download raw datasets or large model files to the user's PC; process them in a Kaggle job and fetch only small outputs. Ask before any download.
- No AGPL code (RustXNG, SearXNG) in or bundled with the app. Check a dependency's licence in its crate or repo before adding it.
- UI automation (`act.ps1`, `retry.ps1` in the scratchpad) only works when Thulli is the foreground window; when the user is active it aborts. Say plainly what was not seen on screen.
- Shell heredocs with nested quotes break here: write scripts with the Write tool; edit CRLF files with scripts that normalise line endings or the Edit tool.
