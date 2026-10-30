# Thulli Product Spec

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
