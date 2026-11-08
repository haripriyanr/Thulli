# Thulli

A calm, private productivity app for your desktop. Focus timer, task spaces, notes and an AI planner that runs entirely on your own device.

Thulli is written in 100% Rust with the [Slint](https://slint.dev) UI toolkit. There is no web view, no account and no cloud service. Your data is encrypted on disk, and the AI never sends anything off your machine.

## What it does

- **Pomodoro** with a task picker, ambient scenes and sounds, a full screen mode and a breathing timer while it runs.
- **Spaces** to organise work as a Board (Backlog, This Week, Today, Done), a priority Matrix and a Calendar. Drag cards between columns, add subtasks, set estimates and dates.
- **Notes** for quick thoughts next to your work.
- **Insights** that show where your focus time went.
- **Plan with AI**: describe what you need in plain words, review the proposed changes, and approve them. The AI can create, edit, complete and delete tasks, subtasks and spaces, and start or stop the timer on a task. Nothing changes until you approve.
- **Voice**: dictate with the microphone and have plans read aloud.
- **115 languages** for the interface, picked by typing in Settings.

## On-device AI

Three models run locally through [ONNX Runtime](https://onnxruntime.ai) (the `ort` crate):

| Job | Model |
|---|---|
| Planning and chat | Qwen3.5-2B Polaris, 4-bit ONNX |
| Speech to text | Omnilingual ASR CTC 300M |
| Read aloud | OmniVoice |

They are packed in one zip (about 2.3 GB) that downloads the first time you use AI, or from **Settings > AI > Download now**. Thulli checks its SHA-256, unpacks it, removes the zip and resumes interrupted downloads. You can remove the models again from the same screen.

## Where things are stored

| OS | App data (encrypted) | AI models |
|---|---|---|
| Windows | `%APPDATA%\thulli\thulli_data.enc` | `%LOCALAPPDATA%\thulli\models` |
| macOS | `~/Library/Application Support/thulli/` | `~/Library/Application Support/thulli/models` |
| Linux | `$XDG_DATA_HOME/thulli/` or `~/.local/share/thulli/` | `.../thulli/models` |

Your data is stored with AES-256-GCM encryption. A file that cannot be read is kept as a timestamped `.bak` copy before Thulli starts fresh.

## Build and run

You need a recent stable [Rust toolchain](https://rustup.rs).

```bash
cargo run --release
```

For development with automatic rebuilds:

```bash
cargo install cargo-watch
cargo watch -x run
```

Run the tests:

```bash
cargo test
```

Try the AI without the UI (downloads the models on first run):

```bash
cargo run --release --example ai_smoke
cargo run --release --example plan_once -- request.txt
```

On Linux you need the usual desktop libraries for Slint and audio (for example ALSA headers).

## Platform status

| OS | Status |
|---|---|
| Windows | Developed and tested here |
| Linux | Builds, tests and AI smoke test pass on a Linux runner |
| macOS | Code paths exist, not yet tested |

The app icon is set for the window and taskbar on all three systems. Packaging metadata for `cargo bundle` is in `Cargo.toml`, and a desktop entry is in `packaging/linux/`.

## Project layout

```
src/
  ai/            on-device models: pack download, planner, speech to text, text to speech
  services/      tasks, timer, notes, preferences, insights, the assistant and its prompts
  models/        saved data types
  repository/    encrypted storage
  data_hub*.rs   connects the services to the UI
  voice.rs       microphone and playback
ui/              Slint screens and reusable components
lang/            translations (one .po file per language)
examples/        command line checks for the AI
```

Contributor and architecture notes live in [AGENTS.md](AGENTS.md).

## Translations

The interface text for all 115 languages was machine drafted and needs review by native speakers. If you find a wrong or awkward phrase, edit `lang/<language>/LC_MESSAGES/thulli.po` and send a pull request. The weakest drafts are CJK variants, Fulfulde, Wolof, Tibetan, Buginese, Banjar, Kashmiri, Sindhi, Meiteilon, Tulu and the regional Arabic variants.

## Known limits

- The planner runs on the CPU, so a long plan can take minutes.
- Very long pasted schedules can confuse the small planner model; shorter requests work best.
- Right-to-left layout mirroring and fonts for every script are not done yet.
