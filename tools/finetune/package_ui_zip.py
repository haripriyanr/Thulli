"""Builds thulli-ui-translation.zip for the UI strings that some language lacks.

  python package_ui_zip.py <out_dir>

The zip holds PROMPT.md, strings.json (English string -> a hint on where it appears),
languages.json and check_ui.py (Gemini runs it on its own output before sending)."""
import json
import subprocess
import sys
import zipfile
from pathlib import Path

HERE = Path(__file__).parent
CONTEXT = {
    "Chat": "Name of a screen and its menu item: a conversation with the assistant. One word.",
    "Chats": "Heading of the list of past conversations.",
    "New chat": "Button that starts a fresh conversation.",
    "Message": "Placeholder text inside the box where you type to the assistant.",
    "Stop": "Link that stops the assistant while it is working. One word.",
    "Yesterday": "Day heading above messages sent the day before today.",
    "Delete this chat?": "Question on a delete confirmation.",
    "Thinking...": "Status while the assistant works out an answer. Keep the three dots.",
    "Asking AI...": "Button label while the assistant is working. Keep the three dots.",
    "Suggest with AI": "Button that asks the assistant for suggestions.",
    "Quick": "One of three choices for how thorough a web search is: Quick, Standard, Deep (shallow to thorough). One word each.",
    "Standard": "Middle choice of Quick / Standard / Deep web search depth.",
    "Deep": "Most thorough choice of Quick / Standard / Deep web search depth.",
    "Planned the searches": "A step in a progress list, past tense: the assistant decided what to look up.",
    "Merging the notes": "A step in a progress list: combining the notes it took from web pages.",
    "Writing the answer": "A step in a progress list: the final answer is being written.",
    "Reading {}": "A step in a progress list. {} is a website address or page title.",
    "Searching: {}": "A step in a progress list. {} is the search words.",
    "Skipped {}": "A step in a progress list. {} is a website address or page title that was not used.",
    "Took notes from {}": "A step in a progress list, past tense. {} is a website address or page title.",
    "{} results": "{} is a number, for example 8 results.",
    "Count my focus hours": "Switch label on a goal: its progress is counted from the hours spent in the focus timer.",
    "Target hours": "Field label on a goal: how many hours to reach.",
    "Let the assistant search the web": "Settings switch label.",
    "When on, what it looks up is sent to the search sites it uses. Everything else stays on this device.": "Settings caption under that switch. Plain, reassuring wording.",
    "Planning, speech recognition and reading aloud run on this device. Only the web searches you allow go online.": "Settings caption under the AI section.",
    "Ask about your tasks, or describe what you want to plan. Nothing changes until you approve it.": "Empty-chat message, friendly tone.",
    # Hover tips: tiny labels that appear when the pointer rests on an icon button. One to three words.
    "Add subtask": "Hover tip on a plus button that adds a smaller step under a task.",
    "Collapse": "Hover tip: fold a board column so only a narrow strip remains.",
    "Expand": "Hover tip: unfold a folded board column.",
    "Collapse menu": "Hover tip on the arrow of the side menu while it is open: make it narrow again.",
    "Expand menu": "Hover tip on the arrow of the side menu: widen it so the names show.",
    "Copy": "Hover tip on a button that copies a chat message.",
    "Click again to delete": "Hover tip on a delete button after the first press: pressing once more deletes for good.",
    "Decrease": "Hover tip on a minus button next to a number in settings.",
    "Increase": "Hover tip on a plus button next to a number in settings.",
    "Delete chat": "Hover tip on a bin button for a whole conversation.",
    "Delete note": "Hover tip on a bin button for a note.",
    "Delete subtask": "Hover tip on a bin button for a smaller step of a task.",
    "Yes, delete": "Hover tip on the tick that confirms deleting a conversation.",
    "Dictate": "Hover tip on the microphone: you speak and it is typed for you.",
    "Stop recording": "Hover tip on the microphone while it is listening.",
    "Full screen": "Hover tip on a button that makes the timer fill the window.",
    "Exit full screen": "Hover tip on a button that leaves the full-screen timer.",
    "Fork from here": "Hover tip on an assistant reply: start a new chat that carries on from this reply.",
    "Fork this chat": "Hover tip: make a copy of the whole conversation as a new one.",
    "Hide subtasks": "Hover tip on a small arrow: hide the smaller steps under a task.",
    "Show subtasks": "Hover tip on a small arrow: show the smaller steps under a task.",
    "Jump to latest": "Hover tip on a round arrow that scrolls the chat to the newest message.",
    "Launch": "Hover tip on a rocket button on a task: puts it first in today's list.",
    "Maximize": "Hover tip on the window's maximize button.",
    "Minimize": "Hover tip on the window's minimize button.",
    "More": "Hover tip on a three-dots button that opens more actions.",
    "Move back": "Hover tip on a left arrow that moves a task to the previous column of the board.",
    "Move forward": "Hover tip on a right arrow that moves a task to the next column of the board.",
    "Move up": "Hover tip on an up arrow that moves a milestone earlier in the list.",
    "Move down": "Hover tip on a down arrow that moves a milestone later in the list.",
    "New space": "Hover tip on a plus button that creates a new space (a named group of tasks).",
    "Next mode": "Hover tip on the right arrow that switches the timer to the next of Focus, Short Break, Long Break.",
    "Previous mode": "Hover tip on the left arrow that switches the timer to the previous of Focus, Short Break, Long Break.",
    "Next version": "Hover tip on an arrow that flips to the newer version of an edited chat message.",
    "Previous version": "Hover tip on an arrow that flips to the older version of an edited chat message.",
    "Next week": "Hover tip on the calendar's right arrow.",
    "Previous week": "Hover tip on the calendar's left arrow.",
    "Undo": "Hover tip on the undo arrow.",
    "Redo": "Hover tip on the redo arrow.",
    "Send": "Hover tip on the button that sends a chat message.",
    "Start": "Hover tip on the timer's play button when the timer is stopped.",
    "Start focus on this task": "Hover tip on a green play button on a task: starts the focus timer for it.",
    "Switch to dark mode": "Hover tip on the sun/moon button in the title bar.",
    "Switch to light mode": "Hover tip on the sun/moon button in the title bar.",
    "Describe what you need and the assistant plans it": "Hover tip on the 'Plan with AI' button. A short sentence is fine here.",
    "See tasks in columns": "Hover tip on the 'Board' tab (columns Backlog, This Week, Today).",
    "Sort tasks by urgency and importance": "Hover tip on the 'Matrix' tab (a 2x2 grid of priorities).",
    "See tasks on a weekly calendar": "Hover tip on the 'Calendar' tab.",
    "A work session": "Hover tip on the timer's 'Focus' tab.",
    "A short rest": "Hover tip on the timer's 'Short Break' tab.",
    "A longer rest": "Hover tip on the timer's 'Long Break' tab.",
    "Set your own length": "Hover tip on the timer's 'Custom' tab: choose how long the timer runs.",
    "Activity for the day": "Hover tip on the 'Day' tab of a focus-time chart.",
    "Activity for the week": "Hover tip on the 'Week' tab of a focus-time chart.",
    "Activity for the month": "Hover tip on the 'Month' tab of a focus-time chart.",
    "Activity for the year": "Hover tip on the 'Year' tab of a focus-time chart.",
    "Move to this week": "Hover tip on a 'Later' button on a task in today's list: moves it to this week.",
    "Go to this week": "Hover tip on the calendar's 'Today' button: jumps back to the current week.",
    "Take it off the calendar": "Hover tip on an 'Unplan' button: removes the task's time from the calendar.",
    "Loading the AI...": "Chat status while the on-device assistant is starting up. Keep the three dots.",
    "Reading your message...": "Chat status while the assistant takes in what you wrote, before it answers. Keep the three dots.",
    "Searching the web...": "Chat status while the assistant looks things up online. Keep the three dots.",
    "Stopped.": "Shown after you press Stop on the assistant's answer.",
    "No changes were made.": "Shown under an assistant answer that talked about changing tasks but proposed nothing you can approve.",
}
PROMPT = """# UI translation job for Thulli

Thulli is a productivity app with an on-device assistant. These short strings appear on its
chat screen, goal form and settings, and as hover tips (tiny labels shown when the pointer rests
on an icon button; the hint says "Hover tip": keep those to one to three words, natural for a
tooltip, no full stop). Translate each English string in `strings.json` into every
language in `languages.json` (`en` is not needed). The value next to each English string in
`strings.json` explains where it appears: use it as context, do not translate it.

Rules:
1. Short, natural, consumer wording. No technical jargon. Same tone as a friendly mobile app.
2. Use each language's standard script. Keep about the same length as the English where you can (these go in buttons).
3. Keep every `{}` placeholder exactly as written, once, in the right place for the grammar of the language. Do not translate or remove it.
4. Keep `...` (three dots) where the English has it.
5. Plain ASCII digits 0-9 if a digit is ever written; no extra quotes; no line breaks inside a string.
6. Return JSON only, one code block per reply, 20 to 30 languages per reply:
   {"<language id>": {"<English string>": "<translation>", ...}, ...}
   The keys inside are the English strings exactly as in `strings.json`.

## Check your work
Save everything you have produced into one folder (one file per language is fine, or one big JSON) and run
`python check_ui.py strings.json <file or folder>`. It prints every problem (a missing string, a lost or
extra `{}`, an empty value, a value left in English where the script differs). Fix and re-run until it
says `problems 0`.
"""
CHECK = r'''"""Usage: python check_ui.py strings.json <file.json | folder of json files>"""
import json, re, sys
from pathlib import Path

src = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
target = Path(sys.argv[2])
files = sorted(target.glob("*.json")) if target.is_dir() else [target]
data = {}
for f in files:
    d = json.loads(f.read_text(encoding="utf-8"))
    data.update(d)
bad = 0
for lang, rows in sorted(data.items()):
    for s in src:
        v = rows.get(s)
        if not isinstance(v, str) or not v.strip():
            print(lang, repr(s)[:40], "missing"); bad += 1; continue
        if v.count("{}") != s.count("{}"):
            print(lang, repr(s)[:40], "placeholder count differs"); bad += 1
        if s.endswith("...") and "..." not in v and "…" not in v:
            print(lang, repr(s)[:40], "three dots lost"); bad += 1
        if "\n" in v:
            print(lang, repr(s)[:40], "line break"); bad += 1
print("languages:", len(data), "| problems", bad)
sys.exit(1 if bad else 0)
'''


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    missing = HERE / "work" / "ui_missing.json"
    subprocess.run([sys.executable, str(HERE / "ui_missing.py"), str(missing)], check=True, capture_output=True)
    data = json.loads(missing.read_text(encoding="utf-8"))
    strings = sorted({s for v in data.values() for s in v})
    unknown = [s for s in strings if s not in CONTEXT]
    if unknown:
        print("no hint for:", unknown)
    z = out / "thulli-ui-translation.zip"
    with zipfile.ZipFile(z, "w", zipfile.ZIP_DEFLATED) as f:
        f.writestr("PROMPT.md", PROMPT)
        f.writestr("strings.json", json.dumps({s: CONTEXT.get(s, "") for s in strings}, ensure_ascii=False, indent=1))
        f.writestr("languages.json", json.dumps([{"id": k} for k in sorted(data)], indent=0))
        f.writestr("check_ui.py", CHECK)
    print(z, len(strings), "strings,", len(data), "languages")


if __name__ == "__main__":
    main()
