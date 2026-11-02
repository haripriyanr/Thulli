#!/usr/bin/env python3
"""Synthetic and templated cases for the Thulli assistant.

Output is one JSON object per line (a "case"): the saved data the prompt is built
from, the user's request and the target reply. The prompt text itself is never
written here. `assistant_eval render` builds it with the app's own `plan_prompt` /
`sort_prompt`, and checks every target with the app's own `parse_reply`.

Rules the targets follow (the app prompt's own wording, made concrete):
  * a day the user names becomes `do_date`; the section follows from it:
    today -> today, later this calendar week -> this_week, anything after -> backlog
  * "next <weekday>" means that weekday in the following calendar week
  * a priority word gives a quadrant (urgent -> do_asap, after lunch / this
    evening -> do_later_today, someday -> leave_till_end); a quadrant fixes the section
  * sorting uses only the visible fields: due within a day -> do_asap, section today
    -> do_later_today, this week or due within 7 days -> do_mid_week, else leave_till_end
  * at most 25 actions, at most 8 steps per task, only the four advertised action types
"""
import argparse
import collections
import datetime as dt
import json
import random
import uuid

from langs import HELD_OUT_PROJECTS, HELD_OUT_TASKS, L, TASK_META

KINDS = {  # kind -> weight
    "tiny": 20, "schedule": 20, "project": 14, "order": 8, "sort": 14,
    "injection": 7, "ambiguous": 5, "empty": 4, "overlong": 4, "limit": 4,
}
MAX_ACTIONS = 25
MAX_STEPS = 8
CAT_SPACE = {"work": 0, "home": 1, "personal": 2}

# Real decompositions in English; other languages use phase words ("Plan: Wedding").
EN_PROJECTS = [
    ("Launch the website", "work", ["Choose a domain and hosting", "Write the page copy", "Design the layout", "Build the pages", "Test on phone and laptop", "Set up analytics", "Publish the site"]),
    ("Plan the wedding", "personal", ["Set the budget", "Draft the guest list", "Book the venue", "Hire a caterer", "Send invitations", "Choose outfits", "Plan the seating"]),
    ("Move to a new apartment", "home", ["Sign the lease", "Book the movers", "Pack the kitchen", "Pack the bedroom", "Change the address", "Set up internet", "Clean the old place"]),
    ("Write the thesis", "personal", ["Agree the outline with the advisor", "Collect the sources", "Write the introduction", "Write the methods", "Analyse the results", "Write the discussion", "Proofread everything", "Submit the final copy"]),
    ("Prepare the quarterly review", "work", ["Gather the numbers", "Update the dashboard", "Write the summary", "Make the slides", "Rehearse the talk"]),
    ("Organise a birthday party", "personal", ["Pick a date", "Invite the guests", "Order the cake", "Buy decorations", "Plan the playlist", "Prepare snacks"]),
    ("Renovate the bathroom", "home", ["Get three quotes", "Choose tiles and fixtures", "Book the plumber", "Clear the room", "Supervise the work", "Final clean"]),
    ("Plan the team offsite", "work", ["Poll the team for dates", "Shortlist venues", "Book travel", "Draft the agenda", "Arrange meals", "Collect feedback afterwards"]),
    ("Learn to play guitar", "personal", ["Buy a guitar", "Learn basic chords", "Practise 15 minutes daily", "Learn a first song", "Record a short clip"]),
    ("Set up a home office", "home", ["Measure the space", "Order a desk and chair", "Run the cables", "Fix the lighting", "Organise storage"]),
]
EN_HELD_OUT_PROJECTS = (8, 9)
EN_FRAMES_TRAIN = ["{add} {t}", "{t}", "I need to {tl}", "Remind me to {tl}", "{t} please", "{add} {t}."]
EN_FRAMES_TEST = EN_FRAMES_TRAIN + ["Don't forget: {t}", "Can you add '{t}'?", "todo: {tl}"]


class Gen:
    def __init__(self, seed, split, taken):
        self.rng = random.Random(seed)
        self.split = split
        self.taken = taken  # keys of cases already produced, to keep splits disjoint
        self.counter = 0

    # ----------------------------------------------------------- basics
    def uid(self):
        return str(uuid.UUID(int=self.rng.getrandbits(128), version=4))

    def now(self, hour_range=(8, 20)):
        rng = self.rng
        d = dt.date(2026, 1, 5) + dt.timedelta(days=rng.randrange(0, 350))
        return dt.datetime(d.year, d.month, d.day, rng.randrange(*hour_range), rng.choice([0, 15, 30, 45]))

    def concept(self, n=None):
        """A task concept index; the test split mixes in the held-out ones."""
        seen = [i for i in range(12) if i not in HELD_OUT_TASKS]
        if self.split == "test" and self.rng.random() < 0.5:
            return self.rng.choice(HELD_OUT_TASKS)
        return self.rng.choice(seen)

    def project_concept(self):
        seen = [i for i in range(4) if i not in HELD_OUT_PROJECTS]
        if self.split == "test" and self.rng.random() < 0.5:
            return self.rng.choice(HELD_OUT_PROJECTS)
        return self.rng.choice(seen)

    def spaces_for(self, lex):
        n = self.rng.choices([0, 1, 2, 3], [1, 1, 3, 5])[0]
        idx = sorted(self.rng.sample(range(3), n))
        return idx, [lex["SP"][i] for i in idx]

    @staticmethod
    def space_of(lex, space_idx, cat):
        i = CAT_SPACE[cat]
        return lex["SP"][i] if i in space_idx else None

    def frame(self, lang, lex, title):
        tl = title[0].lower() + title[1:]
        if lang == "en":
            frames = EN_FRAMES_TEST if self.split == "test" else EN_FRAMES_TRAIN
            return self.rng.choice(frames).format(add=lex["add"], t=title, tl=tl)
        return self.rng.choice(["{add} {t}", "{t}", "{t}", "{add}: {t}"]).format(add=lex["add"], t=title)

    @staticmethod
    def section_for(date, today):
        if date == today:
            return "today"
        if date > today and date.isocalendar()[:2] == today.isocalendar()[:2]:
            return "this_week"
        return "backlog"

    def create(self, lex, title, concept=None, cat=None, mins=None, space_idx=(), section="today",
               quadrant=None, do_date=None, steps=None):
        cat = cat or TASK_META[concept][0]
        mins = mins if mins is not None else TASK_META[concept][1]
        a = {"type": "create_task", "title": title}
        sp = self.space_of(lex, space_idx, cat)
        if sp:
            a["space"] = sp
        a["section"] = section
        if mins:
            a["estimate_mins"] = mins
        if quadrant:
            a["quadrant"] = quadrant
        if do_date:
            a["do_date"] = do_date.isoformat()
        if steps:
            a["steps"] = steps
        return a

    def existing(self, lex, n, space_names, now, avoid=(), sorted_ok=False):
        out = []
        pool = [i for i in range(12) if i not in HELD_OUT_TASKS and i not in avoid]
        for _ in range(n):
            c = self.rng.choice(pool)
            t = {"id": self.uid(), "title": lex["T"][c], "space": self.rng.choice(space_names) if space_names else "",
                 "section": self.rng.choice(["today", "this_week", "backlog"]), "estimate_mins": TASK_META[c][1]}
            if self.rng.random() < 0.3:
                t["deadline"] = (now.date() + dt.timedelta(days=self.rng.randrange(0, 20))).isoformat()
            out.append(t)
        return out

    def case(self, lang, kind, mode, now, spaces, tasks, request, actions, canary=None):
        self.counter += 1
        key = (lang, mode, request, tuple(t["title"] + t["id"][:0] for t in tasks), now.isoformat())
        if key in self.taken:
            return None
        self.taken.add(key)
        c = {"id": f"{self.split}-{lang}-{kind}-{self.counter:05d}", "lang": lang, "kind": kind, "mode": mode,
             "now": now.strftime("%Y-%m-%dT%H:%M"), "spaces": spaces, "tasks": tasks, "request": request,
             "target": {"actions": actions}}
        if canary:
            c["canary"] = canary
        return c

    @staticmethod
    def short(task):
        return task["id"][:6]

    # ----------------------------------------------------------- kinds
    def tiny(self, lang, lex):
        rng = self.rng
        now = self.now()
        sidx, spaces = self.spaces_for(lex)
        tasks = self.existing(lex, rng.choice([0, 0, 1, 2, 4]), spaces, now)
        # Focus on one task that is already in the list.
        if tasks and rng.random() < 0.15:
            t = rng.choice(tasks)
            req = f"{lex['focus']} {t['title']}"
            return self.case(lang, "tiny", "plan", now, spaces, tasks, req, [{"type": "start_focus", "task": self.short(t)}])
        c = self.concept()
        title = lex["T"][c]
        mins = TASK_META[c][1]
        req = self.frame(lang, lex, title)
        quadrant, section = None, "today"
        r = rng.random()
        if r < 0.15:
            req, quadrant = f"{lex['urgent']}: {req}", "do_asap"
        elif r < 0.23:
            req, quadrant, section = f"{lex['someday']}: {req}", "leave_till_end", "backlog"
        if rng.random() < 0.25:
            mins = rng.choice([5, 10, 15, 20, 25, 30, 45, 60, 90])
            req = f"{req} ({mins} {lex['mins']})"
        actions = [self.create(lex, title, c, mins=mins, space_idx=sidx, section=section, quadrant=quadrant)]
        if rng.random() < 0.1 and quadrant is None:
            req = f"{req} {lex['startnow']}"
            actions.append({"type": "start_focus", "task": title})
        return self.case(lang, "tiny", "plan", now, spaces, tasks, req, actions)

    def when(self, lang, lex, now, kind=None):
        """(phrase, do_date, section, quadrant) for a time expression."""
        rng = self.rng
        today = now.date()
        kind = kind or rng.choice(["today", "tomorrow", "nextwd", "indays", "nextwd", "tomorrow", "indays"])
        if kind == "lunch":
            return lex["lunch"], today, "today", "do_later_today"
        if kind == "evening":
            return lex["evening"], today, "today", "do_later_today"
        if kind == "today":
            return lex["today"], today, "today", None
        if kind == "tomorrow":
            d = today + dt.timedelta(days=1)
            return lex["tomorrow"], d, self.section_for(d, today), None
        if kind == "nextwd":
            w = rng.randrange(7)
            monday = today - dt.timedelta(days=today.weekday()) + dt.timedelta(days=7)
            d = monday + dt.timedelta(days=w)
            return lex["nextwd"].format(wd=lex["WD"][w]), d, self.section_for(d, today), None
        n = rng.randrange(2, 15)
        d = today + dt.timedelta(days=n)
        return lex["indays"].format(n=n), d, self.section_for(d, today), None

    def schedule(self, lang, lex):
        rng = self.rng
        n_items = rng.choices([1, 2, 3], [6, 3, 1])[0]
        first = rng.choice(["lunch", "evening", None, None, None])
        now = self.now((8, 12) if first == "lunch" else (7, 17))
        sidx, spaces = self.spaces_for(lex)
        tasks = self.existing(lex, rng.choice([0, 1, 3]), spaces, now)
        parts, actions, used = [], [], set()
        for i in range(n_items):
            c = self.concept()
            while c in used:
                c = self.concept()
            used.add(c)
            title = lex["T"][c]
            phrase, d, sec, quad = self.when(lang, lex, now, first if i == 0 else None)
            if rng.random() < 0.5:
                parts.append(f"{lex['add']} {title} {phrase}" if lang == "en" else f"{title} {phrase}")
            else:
                parts.append(f"{phrase}: {title}")
            actions.append(self.create(lex, title, c, space_idx=sidx, section=sec, quadrant=quad, do_date=d))
        return self.case(lang, "schedule", "plan", now, spaces, tasks, "\n".join(parts) if n_items > 1 else parts[0], actions)

    def order(self, lang, lex):
        rng = self.rng
        now = self.now()
        sidx, spaces = self.spaces_for(lex)
        tasks = self.existing(lex, rng.choice([0, 2]), spaces, now)
        cs = rng.sample([i for i in range(12) if i not in HELD_OUT_TASKS or self.split == "test"], rng.choice([2, 3]))
        titles = [lex["T"][c] for c in cs]
        sep = f", {lex['then']} " if lang != "ja" and lang != "zh" else f"，{lex['then']}"
        req = sep.join(titles)
        if lang == "en":
            req = rng.choice([f"First {titles[0]}", f"{titles[0]}"]) + "".join(f"{sep}{t}" for t in titles[1:])
        actions = [self.create(lex, t, c, space_idx=sidx, section="today" if i == 0 else "this_week", quadrant="do_asap" if i == 0 else None)
                   for i, (t, c) in enumerate(zip(titles, cs))]
        return self.case(lang, "order", "plan", now, spaces, tasks, req, actions)

    def project_actions(self, lang, lex, sidx, n_proj=1):
        rng = self.rng
        acts, titles = [], []
        for _ in range(n_proj):
            if lang == "en" and rng.random() < 0.85:
                held = EN_HELD_OUT_PROJECTS
                pool = [i for i in range(len(EN_PROJECTS)) if i not in held]
                i = rng.choice(held) if self.split == "test" and rng.random() < 0.5 else rng.choice(pool)
                title, cat, steps = EN_PROJECTS[i]
            else:
                pc = self.project_concept()
                title, cat = lex["P"][pc], ["work", "personal", "home", "personal"][pc]
                k = rng.randrange(3, 7)
                steps = [f"{lex['PH'][j]}: {title}" for j in range(k)]
            if title in titles:
                continue
            titles.append(title)
            steps = steps[:MAX_STEPS]
            acts.append(self.create(lex, title, cat=cat, mins=min(480, 30 * len(steps)), space_idx=sidx, section="this_week", steps=steps))
        return titles, acts

    def project(self, lang, lex):
        rng = self.rng
        now = self.now()
        sidx, spaces = self.spaces_for(lex)
        tasks = self.existing(lex, rng.choice([0, 1, 3]), spaces, now)
        if rng.random() < 0.25 and tasks:
            # New step under a task that already exists.
            t = rng.choice(tasks)
            c = self.concept()
            step = lex["T"][c]
            req = f'{lex["add"]} "{step}" {lex["under"]} "{t["title"]}"'
            return self.case(lang, "project", "plan", now, spaces, tasks, req,
                             [{"type": "add_subtask", "task": self.short(t), "title": step}])
        titles, acts = self.project_actions(lang, lex, sidx, rng.choices([1, 2, 3], [7, 2, 1])[0])
        req = "\n".join(self.frame(lang, lex, t) for t in titles)
        return self.case(lang, "project", "plan", now, spaces, tasks, req, acts)

    @staticmethod
    def quadrant_for(t, today):
        due = dt.date.fromisoformat(t["deadline"]) if t.get("deadline") else None
        if due and due <= today + dt.timedelta(days=1):
            return "do_asap"
        if t["section"] == "today":
            return "do_later_today"
        if t["section"] == "this_week" or (due and due <= today + dt.timedelta(days=7)):
            return "do_mid_week"
        return "leave_till_end"

    def sort(self, lang, lex, n=None, kind="sort", inject=False, none_left=False):
        rng = self.rng
        now = self.now()
        today = now.date()
        sidx, spaces = self.spaces_for(lex)
        n = n or rng.choice([2, 3, 4, 5, 6, 8, 10, 12])
        pool = [i for i in range(12) if i not in HELD_OUT_TASKS or self.split == "test"]
        tasks = []
        for _ in range(n):
            c = rng.choice(pool)
            t = {"id": self.uid(), "title": lex["T"][c], "space": rng.choice(spaces) if spaces else "",
                 "section": rng.choice(["today", "this_week", "backlog"]), "estimate_mins": TASK_META[c][1]}
            if rng.random() < 0.55:
                t["deadline"] = (today + dt.timedelta(days=rng.randrange(-1, 21))).isoformat()
            if none_left:
                t["quadrant"] = rng.choice(["do_asap", "do_later_today", "do_mid_week", "leave_till_end"])
            tasks.append(t)
        canary = None
        if inject:
            tasks[rng.randrange(len(tasks))]["title"] = lex["inj"]
            canary = "PWNED"
        # Tasks that already have a quadrant are not shown to the model.
        for _ in range(rng.choice([0, 0, 2])):
            t = {"id": self.uid(), "title": lex["T"][rng.choice(pool)], "space": "", "section": "today", "estimate_mins": 15,
                 "quadrant": "do_asap"}
            tasks.insert(rng.randrange(len(tasks) + 1), t)
        pending = [t for t in tasks if not t.get("quadrant")]
        actions = [{"type": "set_quadrant", "task": self.short(t), "quadrant": self.quadrant_for(t, today)} for t in pending][:MAX_ACTIONS]
        return self.case(lang, kind, "sort", now, spaces, tasks, "", actions, canary)

    def injection(self, lang, lex):
        if self.rng.random() < 0.4:
            return self.sort(lang, lex, kind="injection", inject=True)
        rng = self.rng
        now = self.now()
        sidx, spaces = self.spaces_for(lex)
        tasks = self.existing(lex, rng.choice([1, 2, 4]), spaces, now)
        bad = {"id": self.uid(), "title": lex["inj"], "space": rng.choice(spaces) if spaces else "", "section": "today", "estimate_mins": 0}
        tasks.insert(rng.randrange(len(tasks) + 1), bad)
        c = self.concept()
        title = lex["T"][c]
        actions = [self.create(lex, title, c, space_idx=sidx)]
        return self.case(lang, "injection", "plan", now, spaces, tasks, self.frame(lang, lex, title), actions, "PWNED")

    def ambiguous(self, lang, lex):
        rng = self.rng
        now = self.now()
        sidx, spaces = self.spaces_for(lex)
        r = rng.random()
        if r < 0.5:
            # Two open tasks share a title; the convention is to take the first listed.
            c = self.concept()
            dup = [{"id": self.uid(), "title": lex["T"][c], "space": "", "section": s, "estimate_mins": TASK_META[c][1]}
                   for s in rng.sample(["today", "this_week", "backlog"], 2)]
            tasks = self.existing(lex, 2, spaces, now, avoid=(c,)) + dup
            return self.case(lang, "ambiguous", "plan", now, spaces, tasks, f"{lex['focus']} {lex['T'][c]}",
                             [{"type": "start_focus", "task": self.short(dup[0])}])
        # A time with no task named: nothing to create.
        phrase = self.when(lang, lex, now)[0]
        return self.case(lang, "ambiguous", "plan", now, spaces, self.existing(lex, 2, spaces, now), phrase, [])

    def empty(self, lang, lex):
        rng = self.rng
        now = self.now()
        sidx, spaces = self.spaces_for(lex)
        if rng.random() < 0.2:
            return self.sort(lang, lex, kind="empty", n=rng.choice([2, 4]), none_left=True)
        req = rng.choice(["", "   ", "...", "???", lex["hi"], lex["thanks"], lex["hi"] + "!", lex["thanks"] + " :)"])
        return self.case(lang, "empty", "plan", now, spaces, self.existing(lex, rng.choice([0, 2]), spaces, now), req, [])

    def numbered(self, lex, k, sidx):
        pool = [i for i in range(12) if i not in HELD_OUT_TASKS or self.split == "test"]
        seen = collections.Counter()
        titles, acts = [], []
        for _ in range(k):
            c = self.rng.choice(pool)
            seen[c] += 1
            title = lex["T"][c] if seen[c] == 1 else f"{lex['T'][c]} {seen[c]}"
            titles.append(title)
            acts.append(self.create(lex, title, c, space_idx=sidx))
        return titles, acts

    def overlong(self, lang, lex):
        rng = self.rng
        now = self.now()
        sidx, spaces = self.spaces_for(lex)
        if rng.random() < 0.3:
            junk = rng.choice(["a" * 3000, "!" * 2500, (lex["thanks"] + " ") * 300, "z z " * 700])
            return self.case(lang, "overlong", "plan", now, spaces, [], junk, [])
        titles, acts = self.numbered(lex, rng.randrange(10, 23), sidx)
        noise = f"{lex['hi']}! {lex['thanks']}. " * rng.randrange(40, 70)
        req = noise + f", {lex['then']} ".join(titles) + " " + noise[: rng.randrange(100, 600)]
        return self.case(lang, "overlong", "plan", now, spaces, [], req, acts)

    def limit(self, lang, lex):
        rng = self.rng
        if rng.random() < 0.35:
            return self.sort(lang, lex, kind="limit", n=rng.randrange(27, 41))
        now = self.now()
        sidx, spaces = self.spaces_for(lex)
        titles, acts = self.numbered(lex, rng.randrange(MAX_ACTIONS + 2, MAX_ACTIONS + 10), sidx)
        return self.case(lang, "limit", "plan", now, spaces, [], "\n".join(titles), acts[:MAX_ACTIONS])

    def make(self, lang, kind):
        lex = L[lang]
        for _ in range(20):
            c = getattr(self, kind)(lang, lex)
            if c is not None:
                return c
        return None


def build(split, seed, per_lang, english_extra, langs, taken):
    g = Gen(seed, split, taken)
    kinds, weights = list(KINDS), list(KINDS.values())
    out = []
    for lang in langs:
        n = per_lang + (english_extra if lang == "en" else 0)
        # At least one case of every kind per language, then the weighted rest.
        plan = list(kinds) + g.rng.choices(kinds, weights, k=max(0, n - len(kinds)))
        for kind in plan:
            c = g.make(lang, kind)
            if c:
                out.append(c)
    g.rng.shuffle(out)
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", default="data")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--train-per-lang", type=int, default=140)
    ap.add_argument("--test-per-lang", type=int, default=30)
    ap.add_argument("--english-extra", type=int, default=500, help="extra English training cases")
    ap.add_argument("--langs", default="all")
    a = ap.parse_args()
    langs = list(L) if a.langs == "all" else a.langs.split(",")
    import os
    os.makedirs(a.out, exist_ok=True)
    taken = set()
    # Test is generated first so train can never repeat a test case.
    test = build("test", a.seed + 1000, a.test_per_lang, 0, langs, taken)
    train = build("train", a.seed, a.train_per_lang, a.english_extra, langs, taken)
    for name, rows in (("train_cases.jsonl", train), ("test_cases.jsonl", test)):
        with open(os.path.join(a.out, name), "w", encoding="utf-8", newline="\n") as f:
            for r in rows:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
    for name, rows in (("train", train), ("test", test)):
        kinds = collections.Counter(r["kind"] for r in rows)
        print(f"{name}: {len(rows)} cases in {len({r['lang'] for r in rows})} languages; {dict(sorted(kinds.items()))}")


if __name__ == "__main__":
    main()
