# AGENTS.md — Agent & Developer Guide for Thulli

Welcome to **Thulli**! This document provides authoritative architectural guidelines, constraints, design principles, and operational procedures for all AI coding agents and human contributors working on this repository.

---

## 1. Project Mission & Identity

- **Application Name**: Thulli
- **Repository**: `E:\repos\Thuli`
- **Core Purpose**: A 100% native Rust desktop productivity suite merging structured Kanban task management, smart natural language task entry, focus sprint timers, distraction shields (process and web blocking), and scratchpad notes.
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
- Use only the components defined in [`ui/icons.slint`](file:///e:/repos/Thuli/ui/icons.slint) (`IconBolt`, `IconFolder`, `IconArchive`, `IconShield`, `IconScratchpad`, `IconSettings`, `IconSun`, `IconCalendar`, `IconInbox`, `IconRocket`, `IconPlus`, `IconClose`, `IconTrash`, `IconSearch`, `IconClock`, `IconPlay`, `IconPause`, `IconSkip`).

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

### 4.1. Domain Models (`src/models/mod.rs`)
- Domain entities must be encapsulated structs with rich behavioral methods rather than passive data bags.
- Implement explicit factory constructors (`Task::new()`, `TaskList::new_personal()`, `BlockedApp::new()`).
- Implement state transition methods on structs (`task.toggle_completion()`, `list.toggle_archived()`, `blocked_app.toggle_active()`).
- The root aggregate `AppStateStore` encapsulates collection operations (`add_task()`, `remove_task()`, `toggle_task()`, `remove_list_cascade()`).

### 4.2. Repository Pattern (`src/repository/mod.rs`)
- Persistence is abstracted behind the `StorageRepository` trait:
  ```rust
  pub trait StorageRepository: Send + Sync {
      fn load(&self) -> AppStateStore;
      fn save(&self, store: &AppStateStore) -> Result<(), Box<dyn std::error::Error>>;
  }
  ```
- `EncryptedStorageRepository` implements `StorageRepository` to provide local-first AES-256-GCM encrypted persistence. Business services depend exclusively on the trait interface, enabling easy mocking and testing.

### 4.3. Domain Services (`src/services/mod.rs`)
- Business workflows are encapsulated in dedicated service structs:
  - `TaskService`: Handles natural language parsing, list fallback resolution, and task state mutations.
  - `ListService`: Manages category lifecycles and cascade deletions.
  - `TimerService`: Thread-safe, atomic Pomodoro focus sprint countdown timer.

### 4.4. Platform Abstraction (`src/window/mod.rs`)
- Low-level Win32 unsafe calls are encapsulated behind the safe, modular `WindowManager` interface (`begin_native_drag()`, `minimize()`, `toggle_maximize()`, `exit_cleanly()`).

---

## 5. Reusable UI Library Component Standard (`ui/components/`)

The user interface must never be implemented as a monolithic file. All UI widgets and controls must reside in `ui/components/` with detailed docstrings and configurable public properties so they can be reused across other desktop applications:

```
ui/
├── app.slint                 # High-level orchestrator composing library components
├── theme.slint               # Theme palette, spacing, and border radii
├── types.slint               # Shared struct definitions (TaskModel, ListModel, BlockedAppModel)
├── icons.slint               # Duotone vector icon library
├── window_controls.slint     # Frameless window action buttons
└── components/
    ├── buttons.slint         # ThulliButton, ThulliIconButton (with zero-deadzone TouchArea)
    ├── inputs.slint          # ThulliSearchBar, ThulliTaskInput
    ├── cards.slint           # ThulliListCard, ThulliCreateListCard, ThulliTaskCard
    ├── topbar.slint          # ThulliTopBar (isolated drag region + mode switch)
    ├── sidebar.slint         # ThulliSidebar (search, lists, active links, countdown timer)
    └── modals.slint          # ThulliCreateListModal, ThulliPreferencesModal
```

### 5.1. Component Design Guidelines
1. **Explicit Touch Hit-Testing**: Interactive components must define their outer bounds on `TouchArea` with explicit `width` and `height`, eliminating Slint's `0×0` hit-testing dead zones.
2. **Prop Aliasing & Two-Way Binding**: Inputs and stateful controls expose two-way bound properties (`in-out property <string> text <=> ...`).
3. **Event Callbacks**: Components communicate state changes upward via callbacks (`callback submitted(string)`, `callback clicked()`, `callback toggled()`).

---

## 6. Directory Layout & Module Responsibilities

```
E:\repos\Thuli\
├── Cargo.toml                # Reusable thulli_core library + thulli application binary
├── build.rs                  # Slint compilation build script
├── src/
│   ├── lib.rs                # Library root exporting reusable modules
│   ├── main.rs               # Binary entry point, UI callback binder, event loop
│   ├── models/
│   │   └── mod.rs            # OOP domain models (Task, TaskList, BlockedApp, AppStateStore)
│   ├── repository/
│   │   └── mod.rs            # StorageRepository trait & EncryptedStorageRepository
│   ├── services/
│   │   └── mod.rs            # TaskService, ListService, TimerService
│   ├── parser/
│   │   └── mod.rs            # High-performance natural language task parser
│   ├── security/
│   │   └── mod.rs            # SecurityManager (Argon2id + AES-256-GCM)
│   └── window/
│       └── mod.rs            # WindowManager (Win32 native frameless window management)
└── ui/
    ├── app.slint             # Application root orchestrator
    ├── theme.slint           # Reusable theme palette & typography constants
    ├── types.slint           # Reusable data models
    ├── icons.slint           # Reusable duotone vector icons
    ├── window_controls.slint # Reusable window minimize, maximize, close buttons
    └── components/           # Reusable UI component library
```

---

## 7. Natural Language Task Parser

Located in [`src/parser.rs`](file:///e:/repos/Thuli/src/parser.rs), the smart parser extracts actionable metadata from single-line task descriptions:

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
