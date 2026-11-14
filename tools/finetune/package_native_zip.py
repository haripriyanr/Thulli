"""Builds thulli-native-dataset.zip: a prompt that asks Gemini to write natural
requests in each language, with the answer the assistant should give.

  python package_native_zip.py <out_dir>
"""
import json
import sys
import zipfile
from pathlib import Path

import package_for_gemini

PROMPT = """# Write a synthetic dataset for the Thulli assistant, in many languages

Thulli is a task, goals and focus-timer app. A small on-device AI reads what a person types
and answers with actions. We are teaching it to do that reliably in 115 languages, so we need
many REALISTIC requests written the way real people write in each language, each with the
correct answer.

## What to produce
For every language in `languages.json` (skip `en`) write ONE JSON file named `<language id>.json`
containing a JSON array of exactly 42 examples (6 for each of the 7 domains below). Put each
language in its own code block, or as a separate file in a zip if you can build zips. 4 or 5
languages per reply is fine; if a reply is cut off, continue with the missing examples.

Each example is an object:
{
  "domain": "create_task",
  "request": "what the person types, in the language",
  "tasks": [ {"key":"S1","title":"a task already saved","minutes":30,"section":"today"} ],
  "gold": [ {"type":"create_task","title":"..."} ]
}
- `tasks` is the list of tasks the person already has (0 to 4, keys S1..S4). Use `[]` when the example needs none.
  `title` is in the language, short like a to-do item. `section` is today, this_week or backlog.
- `gold` is the list of actions the assistant must answer with. Use ONLY the actions and fields in `ACTIONS.md`.
- Refer to saved tasks as "@S1", "@S2" ... in `gold` (never by title). The request must mention that task by its title.

## The seven domains (6 examples each)
1. `create_task`: the person adds a task (sometimes two or three). Optional: a section (today / this week / someday), a time estimate, a space name, steps. Add only what the request says.
2. `due_priority`: a new task with a due date and a priority, OR changing the deadline or priority of a saved task. Priority words map to quadrants: urgent/high -> do_asap, medium -> do_mid_week, low -> leave_till_end.
3. `start_focus`: start the focus timer on a saved task, or on a new task named in the request; or stop the timer.
4. `log_time`: record time already spent on a saved task (or a new one: then create_task first, then log_time), optionally for a past day.
5. `reschedule`: move a saved task to another day or time, or to this week / someday, or fix two tasks that clash.
6. `manage`: finish a task, reopen it, delete a task or a space, add a step under a task, change a time estimate, create or rename a space, make a note, edit a title.
7. `guard`: requests where the assistant must do NOTHING: vague ("move it", "do that thing"), repeating a task that already exists in `tasks`, or a general question or chat ("what is the capital of France?", "tell me a joke"). `gold` is `[]`.

## How the requests should read (this is the most important part)
Real people do not write perfect textbook sentences. Vary them a lot:
- some polite and full, some short and blunt, some like a voice message without punctuation, some with typos or missing accents, some with abbreviations or slang, some with English words mixed in the way people in that language really do.
- different verbs and word order, different ways to say "today", "tomorrow", "next week", "urgent", "low priority".
- the task title can sit anywhere in the sentence. Titles must be everyday things (call the dentist, buy rice, finish the report, study for the exam, pay rent, wash the car).
- use names of days and dates only through relative words (today, tomorrow, the day after tomorrow, in 3 days, next week, yesterday, 2 days ago). Do not use weekday names or calendar dates.
- write in the normal script of the language; do not transliterate unless people usually do that when typing.

## Dates and times in `gold` (written as tokens, never as real dates)
- `{d+0}` today, `{d+1}` tomorrow, `{d+2}` the day after tomorrow, `{d+7}` in a week, `{d-1}` yesterday, `{d-2}` two days ago, `{d+N}` N days from now.
- A date with a time: `{d+1@14:00}` (24-hour clock, tomorrow at 14:00).
Use these in `deadline`, `do_date`, `date` and `at`. Convert the person's words carefully: "tomorrow" is `{d+1}`.

## Copy rules
- The title in `create_task` / `add_subtask` and every step must be copied from the request EXACTLY, character for character (same spelling and script). If the request has a different form because of grammar, rephrase the request, not the title.
- Numbers: minutes are whole numbers ("1 hour 30 minutes" = 90). Digits in `gold` are plain ASCII.
- Never invent fields. Never add deletes outside the `manage` domain.

## Check your work
Run `python check_native.py <folder with your files>`. It lists every example that breaks a rule. Fix them and re-run until it says `problems 0`.
"""

ACTIONS = """# Actions you may use in `gold` (exact field names)

{"type":"create_task","title":"...","space":"space name","section":"today|this_week|backlog","estimate_mins":30,"quadrant":"do_asap|do_later_today|do_mid_week|leave_till_end","do_date":"{d+1}","deadline":"{d+3}","steps":["...","..."]}
{"type":"update_task","task":"@S1","title":"...","estimate_mins":30,"section":"today|this_week|backlog","notes":"...","do_date":"{d+1}","deadline":"{d+3}","space":"..."}
{"type":"complete_task","task":"@S1","done":true}        (done:false reopens it)
{"type":"delete_task","task":"@S1"}                      (manage domain only)
{"type":"add_subtask","task":"@S1","title":"..."}
{"type":"set_quadrant","task":"@S1","quadrant":"do_asap|do_later_today|do_mid_week|leave_till_end"}
{"type":"schedule_task","task":"@S1","at":"{d+1@14:00}"}
{"type":"log_time","task":"@S1","minutes":45,"date":"{d-1}"}   (task may be a new title copied from the request; date is optional)
{"type":"create_note","title":"...","body":"..."}
{"type":"create_space","name":"...","color":"#RRGGBB"}
{"type":"rename_space","space":"old name","name":"new name"}
{"type":"delete_space","space":"..."}                    (manage domain only)
{"type":"start_focus","task":"@S1"}                      (or a new task title copied from the request)
{"type":"stop_focus"}

Only include fields the request actually asks for. Leave out the rest.
"""

EXAMPLES = [
    {"domain": "create_task", "request": "Please add a task to renew my passport this week, it takes about 45 minutes.", "tasks": [], "gold": [{"type": "create_task", "title": "renew my passport", "section": "this_week", "estimate_mins": 45}]},
    {"domain": "create_task", "request": "add buy rice and wash the car for today", "tasks": [], "gold": [{"type": "create_task", "title": "buy rice", "section": "today"}, {"type": "create_task", "title": "wash the car", "section": "today"}]},
    {"domain": "due_priority", "request": "new urgent task: submit the tax form, due tomorrow", "tasks": [], "gold": [{"type": "create_task", "title": "submit the tax form", "deadline": "{d+1}", "quadrant": "do_asap"}]},
    {"domain": "due_priority", "request": "move the deadline of finish the report to in 4 days", "tasks": [{"key": "S1", "title": "finish the report", "minutes": 90, "section": "this_week"}], "gold": [{"type": "update_task", "task": "@S1", "deadline": "{d+4}"}]},
    {"domain": "start_focus", "request": "lets start working on study for the exam now", "tasks": [{"key": "S1", "title": "study for the exam", "minutes": 60, "section": "today"}], "gold": [{"type": "start_focus", "task": "@S1"}]},
    {"domain": "log_time", "request": "i spent 1 hour 30 min on call the dentist yesterday", "tasks": [{"key": "S1", "title": "call the dentist", "minutes": 30, "section": "today"}], "gold": [{"type": "log_time", "task": "@S1", "minutes": 90, "date": "{d-1}"}]},
    {"domain": "reschedule", "request": "move pay rent to tomorrow at 14:00", "tasks": [{"key": "S1", "title": "pay rent", "minutes": 15, "section": "this_week"}], "gold": [{"type": "schedule_task", "task": "@S1", "at": "{d+1@14:00}"}]},
    {"domain": "manage", "request": "i finished wash the car", "tasks": [{"key": "S1", "title": "wash the car", "minutes": 30, "section": "today"}], "gold": [{"type": "complete_task", "task": "@S1", "done": True}]},
    {"domain": "guard", "request": "do that thing we talked about", "tasks": [{"key": "S1", "title": "buy rice", "minutes": 15, "section": "today"}], "gold": []},
    {"domain": "guard", "request": "what is the capital of France?", "tasks": [], "gold": []},
]


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    langs = [l for l in package_for_gemini.languages() if l["id"] != "en"]
    here = Path(__file__).parent
    z = out / "thulli-native-dataset.zip"
    with zipfile.ZipFile(z, "w", zipfile.ZIP_DEFLATED) as f:
        f.writestr("PROMPT.md", PROMPT)
        f.writestr("ACTIONS.md", ACTIONS)
        f.writestr("languages.json", json.dumps(langs, ensure_ascii=False))
        f.writestr("examples_en.json", json.dumps(EXAMPLES, ensure_ascii=False, indent=1))
        f.writestr("check_native.py", (here / "native_check.py").read_text(encoding="utf-8"))
    print(z, len(langs), "languages")


if __name__ == "__main__":
    main()
