"""Builds the round 3 data: spec lines for `finetune_data` and example lines for `research_rows`.

  python assemble_round3.py <out_dir>

Reads work/skeletons.json, work/translations.json, work/nat_new/*.json (native examples),
work/pool_out/{pools,massive_rows}.json*, work/research_valid.json, work/research_clean.json.
Writes <out_dir>/specs.jsonl and <out_dir>/research_examples.jsonl.

Every row has a `suite`: templated (translated frames), native (written natively), massive
(real requests, Amazon MASSIVE, CC BY 4.0), research. Splits: train, eval. Held-out native,
MASSIVE and research items never appear in training."""
import json
import random
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import assemble  # noqa: E402

HERE = Path(__file__).parent
W = HERE / "work"
TOKEN = re.compile(r"\{d([+-]\d+)((?:@\d\d:\d\d)?)\}")
ORDER = ["type", "task", "title", "goal", "milestone", "space", "name", "color", "section", "estimate_mins", "quadrant", "do_date", "deadline", "at", "minutes", "date", "done", "notes", "body", "steps"]
# Training rows per language and domain for the translated frames.
QUOTA = {"create_task": 12, "due_priority": 20, "start_focus": 6, "log_time": 12, "reschedule": 12, "manage": 10, "guard": 12, "patterns": 10, "plan": 6}
# Patterns round 3 showed missing in every language (skeletons.py `gaps`), and the
# priority rows that date-shifting was made for: these are picked first.
FIRST = {"high-iso", "important", "do-date-medium", "do-date-high", "do-date-low", "do-and-due", "in-n-days", "after-tomorrow",
         "after-tomorrow-time", "in-n-days-time", "new-yesterday", "new-days-ago", "took-yesterday", "note-body", "small-talk"}
# Research answers were the weakest research step; more of them per language.
RESEARCH_ANSWERS = 10
# Test items whose title holds a duration ("stretch for ten minutes"), so the request has two
# durations or invites an estimate: no single right answer.
BAD_TEST_ITEMS = {"e-log_time-026", "e-due_priority-067", "e-reschedule-071"}
MASSIVE_TRAIN_PER_LANG = 10
MASSIVE_TEST_PER_LANG = 6


def canon(action):
    keys = [k for k in ORDER if k in action] + [k for k in action if k not in ORDER]
    return {k: action[k] for k in keys}


def target_of(gold):
    return json.dumps({"actions": [canon(a) for a in gold["actions"]]}, ensure_ascii=False, separators=(",", ":"))


def with_suite(row, suite, split=None):
    row["suite"] = suite
    if split:
        row["split"] = split
    return row


# ------------------------------------------------------------------ translated frames

def shift_dates(sk, tr, rng):
    """A copy with every date written in the request moved to other days. None when the
    request holds no date token or the date is not used the same way in the answer."""
    tokens = {int(m.group(1)) for m in TOKEN.finditer(tr["request"])}
    if not tokens:
        return None
    gold_text = json.dumps(sk["gold"])
    if any(("{d%+d}" % n) not in gold_text for n in tokens):
        return None
    new = {}
    for n in sorted(tokens):
        choices = [d for d in range(1, 22) if d != n and d not in new.values()]
        new[n] = rng.choice(choices)
    sub = lambda m: "{d%+d%s}" % (new[int(m.group(1))], m.group(2)) if int(m.group(1)) in new else m.group(0)
    sk2 = json.loads(TOKEN.sub(sub, json.dumps(sk)))
    tr2 = json.loads(TOKEN.sub(sub, json.dumps(tr, ensure_ascii=False)))
    sk2["sid"] = sk["sid"] + "d" + str(rng.randrange(10**6))
    return sk2, tr2


def swap_titles(sk, tr, pool, rng):
    """A copy with the task titles replaced by real ones from the language's pool, in the
    request, the stored tasks and the answer. None when a title the answer copies is not
    found literally in the request (an inflected form, say)."""
    keys = [k for k in sk["titles"] if k[0] in "TS" and k in tr["titles"]]
    if not keys:
        return None
    request = tr["request"]
    used = {k for k in keys if "{%s}" % k in json.dumps(sk["gold"]) or ('"@%s"' % k) in json.dumps(sk["gold"])}
    picks = rng.sample(pool, len(keys))
    marks, titles = {}, dict(tr["titles"])
    for k, new in zip(keys, picks):
        old = tr["titles"][k]
        if old in request:
            request = request.replace(old, "\x00%s\x00" % k)
            marks[k] = new
        elif k in used:
            return None
        titles[k] = new
    for k, new in marks.items():
        request = request.replace("\x00%s\x00" % k, new)
    sk2 = json.loads(json.dumps(sk))
    sk2["sid"] = sk["sid"] + "t" + str(rng.randrange(10**6))
    return sk2, {"request": request, "titles": titles}


def pool_titles(events):
    out = [e for e in events if 2 <= len(e) <= 32 and not re.search(r"[\d\"{}@]", e)]
    return out if len(out) >= 40 else []


def templated(lang, sks, tr_lang, pool, out):
    rng = random.Random(f"r3-{lang}")
    cand = {d: [] for d in QUOTA}
    for sk in sks:
        tr = None if lang == "en" else tr_lang[sk["sid"]]
        base_tr = tr or {"request": sk["request_en"], "titles": dict(sk["titles"])}
        if sk["split"] == "eval":
            if sk["sid"] not in BAD_TEST_ITEMS:
                out.append(with_suite(assemble.build(sk, lang, tr), "templated"))
            continue
        cand[sk["domain"]].append((1 if sk["pattern"] in FIRST else 0, assemble.build(sk, lang, tr)))
        if sk["domain"] in ("patterns", "plan"):
            continue
        for _ in range(5 if sk["domain"] == "due_priority" else 2):
            r = shift_dates(sk, base_tr, rng)
            if r:
                cand[sk["domain"]].append((1 if sk["pattern"] in FIRST else 0, assemble.build(r[0], lang, r[1])))
        if pool:
            for _ in range(2):
                r = swap_titles(sk, base_tr, pool, rng)
                if r:
                    cand[sk["domain"]].append((0, assemble.build(r[0], lang, r[1])))
    for d, quota in QUOTA.items():
        rows = cand[d]
        first = [row for flag, row in rows if flag]
        rest = [row for flag, row in rows if not flag]
        rng.shuffle(first)
        rng.shuffle(rest)
        # Base frames first so every original scenario is seen, then the variants.
        base = [r for r in rest if re.fullmatch(r"[^-]+-t-\w+-\d{3}", r["id"])]
        var = [r for r in rest if r not in base]
        take = first[: max(0, quota // 2)] + base[: quota // 2] + var
        seen, picked = set(), []
        for r in take + base + first:
            if r["id"] not in seen and len(picked) < quota:
                seen.add(r["id"])
                picked.append(r)
        for r in picked:
            out.append(with_suite(r, "templated", "train"))


# ------------------------------------------------------------------ native

def native(lang, examples, out):
    rng = random.Random(f"nat-{lang}")
    seen_dom = {}
    for k, ex in enumerate(examples):
        dom = ex["domain"]
        nth = seen_dom.get(dom, 0)
        seen_dom[dom] = nth + 1
        sid = f"n-{dom}-{k:02d}"
        today = assemble.date(2026, 11, 9) + assemble.timedelta(days=rng.randrange(0, 200))
        now = assemble.datetime(today.year, today.month, today.day, rng.choice([8, 9, 10, 13, 15, 17]), rng.choice([0, 15, 30, 45]))
        ids = {t["key"]: assemble.rid(sid, lang, t["key"]) for t in ex.get("tasks", [])}
        ctx = {"today": today, "titles": {}, "ids": ids}
        gold = {"actions": assemble.resolve(ex["gold"], ctx)}
        spaces = list(ex.get("spaces") or [])
        for a in gold["actions"]:
            if a.get("type") in ("rename_space", "delete_space") and a.get("space") and a["space"] not in spaces:
                spaces.append(a["space"])
        if not spaces:
            spaces = [assemble.SPACE_POOL[rng.randrange(len(assemble.SPACE_POOL))]]
        tasks = [{"id": ids[t["key"]], "title": t["title"], "mins": t["minutes"], "section": t["section"], "at": None, "deadline": None, "space": spaces[0]} for t in ex.get("tasks", [])]
        out.append(
            {
                "id": f"{lang}-{sid}",
                "lang": lang,
                "domain": dom,
                "suite": "native",
                "split": "train" if nth < 4 else "eval",
                "now": now.strftime("%Y-%m-%dT%H:%M"),
                "spaces": spaces,
                "goals": [],
                "tasks": tasks,
                "focus": [],
                "request": ex["request"],
                "gold": gold,
                "target": target_of(gold),
            }
        )


# ------------------------------------------------------------------ real MASSIVE requests

def massive(rows, out):
    by = {}
    for r in rows:
        by.setdefault((r["lang"], r["partition"]), []).append(r)
    for (lang, part), rs in sorted(by.items()):
        if part not in ("train", "test"):
            continue
        rng = random.Random(f"massive-{lang}-{part}")
        rng.shuffle(rs)
        for k, r in enumerate(rs[: MASSIVE_TRAIN_PER_LANG if part == "train" else MASSIVE_TEST_PER_LANG]):
            rng2 = random.Random(f"{lang}-{part}-{k}")
            today = assemble.date(2026, 11, 9) + assemble.timedelta(days=rng2.randrange(0, 200))
            now = assemble.datetime(today.year, today.month, today.day, rng2.choice([8, 9, 10, 13, 15, 17]), rng2.choice([0, 15, 30, 45]))
            gold = {"actions": [{"type": "create_task", "title": r["title"]}]}
            out.append(
                {
                    "id": f"{lang}-m-{part}-{k:02d}",
                    "lang": lang,
                    "domain": "create_task",
                    "suite": "massive",
                    "split": "train" if part == "train" else "eval",
                    "now": now.strftime("%Y-%m-%dT%H:%M"),
                    "spaces": [assemble.SPACE_POOL[rng2.randrange(4)]],
                    "goals": [],
                    "tasks": [],
                    "focus": [],
                    "request": r["utt"],
                    "gold": gold,
                    "target": target_of(gold),
                }
            )


# ------------------------------------------------------------------ research

def notes_of(merged):
    return [[int(m.group(1)), m.group(2).strip()] for m in re.finditer(r"\[(\d+)\]\s*(.*?)(?=\s*\[\d+\]\s|\Z)", merged, re.S)]


def research(valid, clean, names, out):
    ids = [s["id"] for s in valid]
    held = {sid for i, sid in enumerate(ids) if i % 12 == 11}
    by = {s["id"]: s for s in valid}

    def rows_for(lang, s, tr, split, kinds):
        q = tr["question"]
        facts = [c["facts"] for c in s["chunks"] if c["facts"]]
        notes = [[i + 1, f] for i, f in enumerate(facts)]
        name = names.get(lang, "English")
        base = {"lang": lang, "split": split}
        if "plan" in kinds:
            out.append({**base, "id": f"{lang}-{s['id']}-plan", "kind": "plan", "language": name, "question": q, "queries": tr["queries"]})
        if "gap" in kinds:
            out.append({**base, "id": f"{lang}-{s['id']}-gap", "kind": "gap", "question": q, "searches": tr["queries"], "notes": notes, "missing": tr["gap_missing"]})
        if "answer" in kinds:
            out.append({**base, "id": f"{lang}-{s['id']}-answer", "kind": "answer", "language": name, "question": q, "notes": notes_of(s["merged"]), "answer": tr["answer"]})
        if "extract" in kinds:
            for j, c in enumerate(s["chunks"]):
                out.append({**base, "id": f"{lang}-{s['id']}-extract{j}", "kind": "extract", "question": q, "chunk": c["text"], "facts": c["facts"]})
        if "merge" in kinds:
            out.append({**base, "id": f"{lang}-{s['id']}-merge", "kind": "merge", "notes": notes, "merged": s["merged"]})

    en = lambda s: {"question": s["question"], "queries": s["queries"], "gap_missing": s["gap_missing"], "answer": s["answer"]}
    rng = random.Random("research")
    train_ids = [i for i in ids if i not in held]
    # English: all of the page-reading steps (they are language independent), and the rest for a part.
    for sid in rng.sample(train_ids, 70):
        rows_for("en", by[sid], en(by[sid]), "train", ["extract", "merge"])
    for sid in rng.sample(train_ids, 40):
        rows_for("en", by[sid], en(by[sid]), "train", ["plan", "gap", "answer"])
    for sid in sorted(held):
        rows_for("en", by[sid], en(by[sid]), "eval", ["extract", "merge", "plan", "gap", "answer"])
    for lang, rows in sorted(clean.items()):
        have = [sid for sid in rows if sid in by]
        tr_ids = [sid for sid in have if sid not in held]
        ev_ids = sorted(sid for sid in have if sid in held)
        r = random.Random(f"research-{lang}")
        picked = r.sample(tr_ids, min(RESEARCH_ANSWERS, len(tr_ids)))
        for i, sid in enumerate(picked):
            rows_for(lang, by[sid], rows[sid], "train", ["plan", "gap", "answer"] if i < 4 else ["answer"])
        for sid in r.sample(ev_ids, min(2, len(ev_ids))):
            rows_for(lang, by[sid], rows[sid], "eval", ["plan", "gap", "answer"])


def main():
    out_dir = Path(sys.argv[1])
    out_dir.mkdir(parents=True, exist_ok=True)
    import os
    sks = json.loads((W / os.environ.get("SKELETONS", "skeletons.json")).read_text(encoding="utf-8"))
    trs = json.loads((W / os.environ.get("TRANSLATIONS", "translations.json")).read_text(encoding="utf-8"))
    pools = json.loads((W / "pool_out" / "pools.json").read_text(encoding="utf-8"))
    rows = []
    for lang in ["en"] + sorted(trs):
        templated(lang, sks, trs.get(lang), pool_titles(pools.get(lang, {}).get("events", [])) if lang != "en" else [], rows)
    n_templated = len(rows)
    for f in sorted((W / "nat_new").glob("*.json")):
        native(f.stem, json.loads(f.read_text(encoding="utf-8")), rows)
    n_native = len(rows) - n_templated
    m = [json.loads(l) for l in (W / "pool_out" / "massive_rows.jsonl").read_text(encoding="utf-8").splitlines() if l.strip()]
    massive(m, rows)
    n_massive = len(rows) - n_templated - n_native
    with open(out_dir / "specs.jsonl", "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    valid = json.loads((W / "research_valid.json").read_text(encoding="utf-8"))
    clean = json.loads((W / "research_clean.json").read_text(encoding="utf-8"))
    names = {l["id"]: l["name"] for l in json.loads((W / "languages.json").read_text(encoding="utf-8"))}
    names["en"] = "English"
    ex = []
    research(valid, clean, names, ex)
    with open(out_dir / "research_examples.jsonl", "w", encoding="utf-8") as f:
        for r in ex:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    tr = sum(1 for r in rows if r["split"] == "train")
    print(f"specs {len(rows)} (templated {n_templated}, native {n_native}, massive {n_massive}); train {tr}, eval {len(rows) - tr}; research examples {len(ex)}")


if __name__ == "__main__":
    main()
