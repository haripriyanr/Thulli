# Thulli Product Spec

> **Historical specification:** the product screens and navigation described
> below were removed from the application at the user's direction on 2026-11-02.
> The current build is a screen-free shell; this file records the former target
> behavior only and must not be treated as the current UI.

Target behaviour for Thulli, written from hands-on use of comparable desktop
focus apps. This file describes *behaviour only*. Layout, icons, colours and all
user-facing wording are Thulli's own and follow `AGENTS.md`.

Status tags: **[seen]** used directly, **[listed]** visible but not exercised
(paywalled or needs an outside account), **[assumed]** inferred, confirm on review.

---

## 1. Navigation shell

- Three primary tabs in a bottom pill: **Today**, **Tasks**, **Insights**. [seen]
- Top bar: "..." app menu, workspace name, profile menu, window controls. [seen]
- A small running-timer pill stays visible on every screen while time is tracked. [seen]
- Back / forward navigation history: `Alt+Left`, `Alt+Right`; home: `Ctrl+Shift+,`. [seen]
- View commands: zoom in, zoom out, actual size, centre window, full screen (`Ctrl+F`
  conflicts with search; use `F11`). [seen]
- Global search, `Ctrl+F`: finds tasks and lists; offers quick actions add task,
  add list, go to Insights. [seen]

## 2. Daily workflow

The day moves through **Plan → Work → Wrap up**.

### Plan [seen]
- Today shows an empty state offering **Plan your day** or **Skip planning**.
- Planning opens a split screen: task sidebar plus Today / Sooner / Later sections,
  with a **Finish planning** action.
- Planning time is itself tracked and appears on the day timeline as a "Planning" block.

### Work: focus sessions [seen]
- Session bar: duration picker (10 to 180 min in 5 min steps, default 90),
  **Start session**.
- A session runs a countdown (MM:SS) with pause / resume. While paused,
  **Finish session** appears.
- Tasks can be started without choosing one, and a focus task can be chosen at any
  time from the priority list or a task picker; time then accrues to that task.
- Focus view shows the task, its notes, its breakdown steps, and actions
  **Waiting**, **Complete**, **Switch task**.
- Finishing a session under 1 minute prompts: cancel, save, or delete session.
- A collapse control shrinks the session to a compact view.
- After a session, optional pause suggestions appear (setting). [listed]

### Sprint mode and breaks [listed]
- Optional pomodoro: work sprint length and break length, default break length.
- Actions: start break, pause task, skip task, finish active task, add notes to
  active task. Each has a shortcut (see 11).
- Task time alerts: alert every N minutes during a task, with a chosen sound and an
  optional flash animation on the timer.
- Title-bar text scrolls the live timer (optional).
- Always-on-top compact timer panel, docked left or right on a chosen screen.

### Wrap up [listed]
- **Wrap up early** ends the working day. A brain-dump step clears the mind before
  closing the day. [assumed]

### Daily goal [seen]
- Goal ring in the header shows progress towards the daily goal.
- Its popover shows total goal (editable), worked, left, and a breakdown of focused
  work, meetings and other.
- Settings: daily goal, count weekends toward goals, derived weekly goal.

## 3. Tasks

- Sections: **Today**, **Sooner**, **Later**, each with an inline add row. [seen]
- Sidebar: **All tasks** (with count), **Logbook** (completed tasks and projects),
  **Waiting**, **Projects** (add). [seen]
- Tasks nest: breakdown steps and sub-steps, to at least three levels, plus note
  lines (bullets, links) under a task. [seen]
- Context menu and single-key shortcuts [seen]:

| Action | Key |
| :--- | :--- |
| Select task to focus on | `S` |
| Select (multi-select) | `X` |
| Mark completed | `C` |
| Mark cancelled | `Shift+C` |
| Mark waiting | `W` |
| Assign project | `P` |
| Open task | `O` |
| Move to top / up / down / bottom | `Alt+Shift+Up`, `Alt+Up`, `Alt+Down`, `Alt+Shift+Down` |
| Remove from Today | `Shift+T` |
| Edit notes | `Shift+Enter` |
| Edit name | `E` |
| Copy | submenu |
| New task | `Enter` |
| Add breakdown step | `Ctrl+Enter` |
| Delete (with confirmation) | `Del` |

- Natural language entry: when enabled, parse time estimates from the title. A
  preference turns this off. Our parser in `src/parser.rs` already does this. [seen]
- A task can carry a schedule; a reminder fires N minutes before it. [listed]
- Estimated and done times show on tasks; a preference hides them. [listed]
- Completing a task can show a success screen with a playful animation and a sound,
  each switchable. [listed]

## 4. Lists

- Home grid of list cards: letter badge or uploaded icon, name, menu, pending count,
  and an all-clear state; hover reveals **Open**. [seen]
- List menu: edit, duplicate, archive. [seen]
- Create list: optional icon upload (jpg, png, svg), colour from a palette, title. [seen]
- Archive area with tabs for archived lists and archived done tasks. [seen]
- A list opens as a board. [listed]

Lists and Projects are one concept in Thulli: `TaskList`. [assumed]

## 5. Notes and knowledge

- **Notes**: folders, an Unsorted bucket, Trash, new note and new folder. [seen]
- Note header shows a timestamp and a backlink count. [seen]
- Rich text: headings, bullets (Tab to indent, multi-level), numbered lists, inline
  code, code blocks, checklists, link cards, slash command menu. [seen]
- Editor commands: create task from selection (`Ctrl+Shift+O`), add link
  (`Ctrl+Alt+K`), toggle todo (`Ctrl+Enter`), collapse item, spell check toggle. [seen]
- **Quick Capture**: scratch inbox with a global shortcut (`Alt+,`). [seen]
- **Floating Notes**: always-on-top side panel, global shortcut (`Alt+.`); shows notes
  for the current task. [seen]
- **Meeting Notes**: collects notes captured during meetings. [seen]
- **Brag Book**: dated accomplishments list with an add row. [seen]

## 6. Calendar

- Right-hand day timeline with hour rows, a live now-line, and tracked blocks for
  planning and focus sessions. [seen]
- Day header with date; the panel collapses. [seen]
- External calendar events and Meeting Mode need outside accounts: see 10.
- Setting: week start day. [seen]

## 7. Insights

- Period navigator (this week, previous, next), project filter, CSV export. [seen]
- Cumulative worked-time chart against a goal-pace line, with an "on pace" readout. [seen]
- Tiles: time ratio (focus, meetings, other), per-workday bars, consistency heatmap
  with streak. [seen]
- Tasks worked on, filter all / done / in progress. [seen]
- Activity log per day, each entry editable. [seen]
- Reports: list filter, date range, totals (work days, tasks done, time worked,
  average time per task), tasks / breaks / total chart, most productive hour / day /
  month, time by list, done tasks on-time ratio, sessions tab, PDF export. [seen]

## 8. Distraction shield

- Block chosen apps and websites during sessions. [listed]
- Pure-Rust Windows approach:
  - Apps: watch processes with `sysinfo`; close blocked processes while a session runs.
  - Websites: managed section of the system hosts file. This needs elevation, so
    Thulli must explain that plainly and work without it.

## 9. Assistant (local chat)

- Floating launcher opens a chat panel with new chat, history, close, text input,
  microphone button. [seen]
- Turns free-form text into tasks, notes, subtasks, and schedules. [seen]
- Thulli runs this entirely on the device with the `mistralrs` crate, behind a trait
  so tests use a mock. Nothing leaves the machine.
- Model file: user-chosen GGUF path in preferences; Thulli never downloads silently.
- Voice input: out of scope for the first version.

## 10. Accounts and integrations (excluded from first version)

Thulli is local-first. These need outside accounts or services and are **not built**
in this pass: paid plans, trials, sign-in, workspaces beyond one local workspace,
cloud sync, team features, Google / Apple calendar, Slack status, Linear, Jira,
Notion, ClickUp, Asana, Zapier, MCP server, REST API, webhooks, Loom and video embeds.

## 10a. Additions from the newer app generation

All **[seen]** unless tagged. These extend the sections above; where they differ,
this section wins.

### Board, one task model
- Lists and tasks open in three views: **Board**, **Matrix**, **Calendar**.
- Board columns: **Backlog, This Week, Today, Done**, each column with a progress bar
  (n/m done), add task, and a collapse control. Today is highlighted.
- These are the same sections as Later, Sooner, Today in section 3. Stored values:
  `backlog`, `this_week`, `today`, plus completed status. Thulli shows one vocabulary:
  Backlog, This Week, Today, Done.
- Drag tasks between columns. Undo and redo for board actions.
- Sidebar: search, list folders, add list, **All Lists** aggregate view, Shared
  (excluded), Archive, Reports, Settings; collapses to an icon rail.
- Filter menu with a Dates filter.
- Mic button beside Add task for voice capture (excluded for now).

### Task fields and editing
- Inline card editor: title, time taken, estimate, tags, Do Date, Deadline, subtasks,
  notes, with Save and Esc.
- Hover actions on a card: move, start focus, edit, menu. Menu: ask assistant about
  this task, history, delete.
- Tags: reusable, with optional emoji and colour; managed in settings.

### Matrix view
- Four quadrants: Do ASAP (today, urgent and important), Do Later Today (today,
  important), Do Mid Week (this week, important), Leave Till End (backlog, low value).
- Each quadrant has a Review action and a count; drag tasks in.
- Left rail: Matrix Review with a "prioritize one by one" flow and an Unsorted queue
  grouped by Today, This Week, Backlog.

### Calendar view
- Unscheduled rail grouped by section; drag a task onto the grid, or click a slot to
  create one. Day, Week, Month, Today, previous and next.

### Focus run
- Start from a card, or from a global **start now** action that is enabled when Today
  has tasks. Starting moves the task to Today.
- Three display modes: **Expanded** (full window, ring timer), **Tower** (docked
  screen-edge panel), **Pill** (tiny floating timer). Switch by shortcut.
- The timer counts **up** on a task (HH:MM:SS) against its estimate.
- Tower: queue of Today's tasks, n/m done, add task, close session. Hover on the active
  task reveals done, break, notes, pause, subtasks, skip.
- Expanded: ring, time vs estimate, add subtasks, break, pause, notes, skip, Done, and a
  background picker with presets plus your own image.
- Break: starts a 5 minute countdown (default) and puts the task back in the queue.
- Options: tower side, move freely, auto-hide at the screen edge, expanded as full
  screen or maximized; pomodoro on or off; start next task when a break ends early.
- Overtime alert when a task exceeds its estimate.

### Assistant additions
- Always-available **Ask anything** bar with a day summary, recent chats, and actions
  Wind-down Reflection and Quick Prioritize. Action results **[listed]**.
- Memory: facts the assistant keeps, each typed as preference, commitment, person or
  project, or pattern. The user can add, edit, remove, and see history.
- Suggestions, each switchable, shown in-app only and never during focus: plan my day,
  welcome back, take a break, wind down, prioritize.
- Voice call and wake word: excluded for now.

### Reports additions
- Quick ranges 7D, 30D, 90D, custom; daily activity bars for tasks, breaks, total;
  time by list donut; done tasks.
- Sessions tab: every task and break session with list, date, start, end, duration;
  show or hide breaks; add a session by hand; edit via row menu; CSV export.

### Settings additions
- Language; open on wake and login; show full task title (wrap).
- Colour mode plus five accent themes, with a live preview of dashboard, tower and pill.
- Sounds: notification, break start, task start, overtime, schedule reminders (minutes
  before), break reminders (every N minutes, own sound), timed task alerts, animated
  pill alert.
- Celebration: success screen, animation on success and on break end, separate sets for
  on-time and over-estimate. Thulli ships its own artwork; no third-party GIFs.
- Every shortcut can be enabled, disabled and rebound. New: dashboard, new list, search,
  find pill, preferences, next task, switch tower and pill.
- First-run checklist with coach tips.

### Still excluded
Sign-in, plans, sharing, the CLI, tokens, outbound webhooks, MCP and all outside
integrations remain out of scope per section 10.

## 11. Preferences and shortcuts

- Appearance: theme system / dark / light. [seen]
- Time: timezone. [seen]
- General: hide estimates, auto-parse estimates, open at login. [seen]
- Session: default length, sprint settings, pause suggestions. [seen]
- Alerts: see 2 and 3; sound pickers with preview. [seen]
- Celebration: success screen, animation, sound. [seen]
- Compact panel: screen and side. [seen]
- Global shortcuts, each with an enable toggle [seen]: show app, toggle focus mode,
  find focus timer, floating notes, quick capture.
- App shortcuts [seen]: new task `Ctrl+Alt+T`, start break `Ctrl+Alt+B`, pause
  `Ctrl+Alt+P`, skip `Ctrl+Alt+S`, finish `Ctrl+Alt+F`, add notes `Ctrl+Alt+N`,
  search `Ctrl+F`.
