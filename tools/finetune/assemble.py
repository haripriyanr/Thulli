"""Builds spec lines (one per language and scenario) from skeletons and
translations, for `cargo run --example finetune_data`.

  python assemble.py work/skeletons.json work/translations.json work/specs.jsonl
"""
import hashlib
import json
import random
import re
import sys
from datetime import date, datetime, timedelta
from pathlib import Path

DATE = re.compile(r"\{d([+-]\d+)(?:@(\d\d:\d\d))?\}")
SPACE_POOL = ["Work", "Study", "Home", "Personal"]


def rid(*parts):
    return hashlib.sha1("|".join(parts).encode()).hexdigest()[:8]


def resolve(value, ctx):
    """Replace placeholders in a string, list or dict."""
    if isinstance(value, str):
        if value.startswith("@S") and value[1:] in ctx["ids"]:
            return ctx["ids"][value[1:]][:6]

        def sub(m):
            d = ctx["today"] + timedelta(days=int(m.group(1)))
            return f"{d.isoformat()}T{m.group(2)}" if m.group(2) else d.isoformat()

        value = DATE.sub(sub, value)
        for k, v in ctx["titles"].items():
            value = value.replace("{" + k + "}", v)
        return value
    if isinstance(value, list):
        return [resolve(v, ctx) for v in value]
    if isinstance(value, dict):
        return {k: resolve(v, ctx) for k, v in value.items()}
    return value


def build(sk, lang, tr):
    rng = random.Random(rid(sk["sid"], lang))
    today = date(2026, 11, 9) + timedelta(days=rng.randrange(0, 200))
    now = datetime(today.year, today.month, today.day, rng.choice([8, 9, 10, 13, 15, 17]), rng.choice([0, 15, 30, 45]))
    titles = tr["titles"] if tr else dict(sk["titles"])
    ids = {t["key"]: rid(sk["sid"], lang, t["key"]) for t in sk["store"]}
    ctx = {"today": today, "titles": titles, "ids": ids}
    request = resolve(tr["request"] if tr else sk["request_en"], ctx)
    spaces = [resolve(titles[k], ctx) if k in titles else k for k in sk["spaces"]]
    space_names = spaces or [SPACE_POOL[rng.randrange(len(SPACE_POOL))]]
    tasks = []
    for t in sk["store"]:
        tasks.append(
            {
                "id": ids[t["key"]],
                "title": titles[t["key"]],
                "mins": t["mins"],
                "section": t["section"],
                "at": resolve(t["at"], ctx) if t["at"] else None,
                "deadline": resolve(t["deadline"], ctx) if t["deadline"] else None,
                "space": space_names[0],
            }
        )
    goals = [titles[k] for k in sk["goals"] if k in titles]
    focus = [{"date": (today - timedelta(days=f["days_ago"])).isoformat(), "hour": f["hour"], "mins": f["mins"]} for f in sk["focus"]]
    gold = resolve(sk["gold"], ctx)
    return {
        "id": f"{lang}-{sk['sid']}",
        "lang": lang,
        "domain": sk["domain"],
        "split": sk["split"],
        "now": now.strftime("%Y-%m-%dT%H:%M"),
        "spaces": space_names,
        "goals": goals,
        "tasks": tasks,
        "focus": focus,
        "request": request,
        "gold": gold,
        "target": json.dumps(gold, ensure_ascii=False, separators=(",", ":")),
    }


def main():
    sks = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    trs = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
    n = 0
    with open(sys.argv[3], "w", encoding="utf-8") as out:
        for lang in ["en"] + sorted(trs):
            for sk in sks:
                tr = None if lang == "en" else trs[lang][sk["sid"]]
                out.write(json.dumps(build(sk, lang, tr), ensure_ascii=False) + "\n")
                n += 1
    print(n, "specs")


if __name__ == "__main__":
    main()
