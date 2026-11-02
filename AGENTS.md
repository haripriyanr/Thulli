# AGENTS.md — Agent & Developer Guide for Thulli

Welcome to **Thulli**! This document provides authoritative architectural guidelines, constraints, design principles, and operational procedures for all AI coding agents and human contributors working on this repository.

---

## 1. Project Mission & Identity

- **Application Name**: Thulli
- **Repository**: `E:\repos\Thuli`
- **Core Purpose**: A 100% native Rust desktop shell with local encrypted storage and reusable Slint UI components. Product screens and navigation are currently removed at the user's direction; the productivity domain services remain in `thulli_core`.
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
- Screens (Today, Pomodoro, Tasks, Notes, Insights, Settings) are restored and show only saved data. Every value they display comes from `AppData` (`ui/app_data.slint`), which Rust fills from the saved state in `src/data_hub.rs`; display rows are built in `src/convert.rs`.
- Screens never hold sample or invented data. A screen with nothing saved shows an empty-state line.
- `DataHub` changes data only through the services (`TaskService`, `NotesService`, `PreferencesService`, `StoreHandle`) so every change is saved. Do not write to the store from a screen.

---

## 5. Reusable UI Library Component Standard (`ui/components/`)

The user interface must never be implemented as a monolithic file. The current `app.slint` is a screen-free native shell; reusable widgets remain in `ui/components/` with configurable public properties. Product screens are intentionally absent unless the user asks to restore them.

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
