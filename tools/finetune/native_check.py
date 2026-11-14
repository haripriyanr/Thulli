"""Checks natively written synthetic examples (from Gemini).

  python native_check.py <file.json | folder of <lang>.json> [--strict-count N]

Each example: {"domain","request","tasks":[{"key","title","minutes","section"}],
"spaces":[..]?, "gold":[actions]}. Prints every problem; exit code 1 if any."""
import json
import re
import sys
from pathlib import Path

DOMAINS = ["create_task", "due_priority", "start_focus", "log_time", "reschedule", "manage", "guard"]
SECTIONS = {"today", "this_week", "backlog"}
QUADRANTS = {"do_asap", "do_later_today", "do_mid_week", "leave_till_end"}
DATE = re.compile(r"^\{d[+-]\d+\}$")
DATETIME = re.compile(r"^\{d[+-]\d+@\d\d:\d\d\}$")
ALLOWED = {
    "create_task": {"type", "title", "space", "section", "estimate_mins", "quadrant", "do_date", "deadline", "steps"},
    "update_task": {"type", "task", "title", "estimate_mins", "section", "notes", "do_date", "deadline", "space"},
    "complete_task": {"type", "task", "done"},
    "delete_task": {"type", "task"},
    "add_subtask": {"type", "task", "title"},
    "set_quadrant": {"type", "task", "quadrant"},
    "schedule_task": {"type", "task", "at"},
    "log_time": {"type", "task", "minutes", "date"},
    "create_note": {"type", "title", "body"},
    "create_space": {"type", "name", "color"},
    "rename_space": {"type", "space", "name"},
    "delete_space": {"type", "space"},
    "start_focus": {"type", "task"},
    "stop_focus": {"type"},
}
DELETES = {"delete_task", "delete_space"}


def norm(s):
    return " ".join(str(s).split()).casefold()


def check(ex):
    errs = []
    d, req = ex.get("domain"), ex.get("request", "")
    if d not in DOMAINS:
        return [f"unknown domain {d!r}"]
    if not isinstance(req, str) or not (3 <= len(req) <= 400):
        errs.append("request length")
    tasks = ex.get("tasks", [])
    keys = []
    for t in tasks:
        if not re.fullmatch(r"S[1-4]", str(t.get("key", ""))) or t["key"] in keys:
            errs.append("task key")
        keys.append(t.get("key"))
        if not str(t.get("title", "")).strip() or len(t["title"]) > 80:
            errs.append("task title")
        if not isinstance(t.get("minutes"), int) or not (5 <= t["minutes"] <= 240):
            errs.append("task minutes")
        if t.get("section") not in SECTIONS:
            errs.append("task section")
    if len(tasks) > 4:
        errs.append("too many tasks")
    gold = ex.get("gold")
    if not isinstance(gold, list):
        return errs + ["gold must be a list"]
    if d == "guard":
        if gold:
            errs.append("guard gold must be empty")
        return errs
    if not gold:
        errs.append("gold is empty")
    for a in gold:
        t = a.get("type")
        if t not in ALLOWED:
            errs.append(f"unknown action {t!r}")
            continue
        if set(a) - ALLOWED[t]:
            errs.append(f"{t}: extra keys {sorted(set(a) - ALLOWED[t])}")
        if t in DELETES and d != "manage":
            errs.append("deletes only belong in manage")
        for k in ("task",):
            v = a.get(k)
            if v is None:
                continue
            if re.fullmatch(r"@S[1-4]", str(v)):
                if v[1:] not in keys:
                    errs.append(f"{t}: {v} is not a task of this example")
                elif d not in ("reschedule",) and norm(next(x["title"] for x in tasks if x["key"] == v[1:])) not in norm(req):
                    errs.append(f"{t}: task title must appear in the request")
            elif t in ("start_focus", "log_time"):
                if norm(v) not in norm(req):
                    errs.append(f"{t}: a new task title must appear in the request")
            else:
                errs.append(f"{t}: task must be @S<n>")
        for k in ("title",):
            if k in a and t in ("create_task", "add_subtask") and norm(a[k]) not in norm(req):
                errs.append(f"{t}: title must be copied from the request")
        for k in ("do_date", "deadline", "date"):
            if k in a and not DATE.match(str(a[k])):
                errs.append(f"{t}: {k} must look like {{d+3}}")
        if "at" in a and not DATETIME.match(str(a["at"])):
            errs.append("schedule_task: at must look like {d+1@14:00}")
        if "quadrant" in a and a["quadrant"] not in QUADRANTS:
            errs.append("quadrant")
        if "section" in a and a["section"] not in SECTIONS:
            errs.append("section")
        for k in ("estimate_mins", "minutes"):
            if k in a and (not isinstance(a[k], int) or not (1 <= a[k] <= 600)):
                errs.append(f"{k} must be whole minutes")
        if "steps" in a and (not isinstance(a["steps"], list) or any(norm(s) not in norm(req) for s in a["steps"])):
            errs.append("steps must be copied from the request")
        if t == "create_task" and "task" in a:
            errs.append("create_task has no task field")
    if d == "start_focus" and not any(a.get("type") in ("start_focus", "stop_focus") for a in gold):
        errs.append("start_focus domain needs start_focus or stop_focus")
    if d == "log_time" and not any(a.get("type") == "log_time" for a in gold):
        errs.append("log_time domain needs log_time")
    if d == "reschedule" and not any(a.get("type") in ("schedule_task", "update_task") for a in gold):
        errs.append("reschedule domain needs schedule_task or update_task")
    return errs


def main():
    target = Path(sys.argv[1])
    want = int(sys.argv[sys.argv.index("--strict-count") + 1]) if "--strict-count" in sys.argv else 0
    files = sorted(target.glob("*.json")) if target.is_dir() else [target]
    bad = good = 0
    for f in files:
        data = json.loads(f.read_text(encoding="utf-8"))
        rows = data if isinstance(data, list) else next(iter(data.values()))
        ok = 0
        for i, ex in enumerate(rows):
            e = check(ex)
            if e:
                bad += 1
                print(f.name, i, ex.get("domain"), e)
            else:
                good += 1
                ok += 1
        if want and ok < want:
            print(f.name, f"only {ok} valid examples, wanted {want}")
    print(f"valid {good}, problems {bad}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
