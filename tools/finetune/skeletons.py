"""English scenario skeletons for the planner fine-tune.

Each skeleton holds an English request, the titles it uses (a translator
renders them per language), the saved state the prompt will show, and the
gold answer as a template. The translator never writes JSON: titles and the
request come back, and `assemble.py` builds the gold answer mechanically.

Placeholders:
  {T1}        a translated title (also S1.. for tasks already saved, G1 goal, M1.. milestones)
  {d+3}       the date three days after the example's "now" (ISO, resolved later)
  {d+1@14:00} a date and time
  @S1         the id of saved task S1

Run:  python skeletons.py out_dir     (writes skeletons.json)
"""
import json
import random
import sys

MINS = [15, 20, 25, 30, 45, 60, 90, 120]

TRAIN_TASKS = [
    "write the project report", "call the dentist", "buy groceries", "prepare the slides", "clean the kitchen",
    "read chapter five", "book the train tickets", "pay the electricity bill", "water the plants", "review the budget",
    "email the landlord", "update my resume", "plan the team meeting", "practice the guitar", "fix the bicycle",
    "renew the passport", "study for the chemistry quiz", "do the laundry", "back up my photos", "write the thank you cards",
    "learn ten new words", "organize my desk", "submit the tax form", "visit the library", "cook dinner for friends",
    "finish the lab notes", "schedule a doctor visit", "research laptops", "edit the video", "sort the mail",
    "prepare the presentation", "go for a run", "revise the essay", "order printer ink", "call my parents",
    "clean the garage", "write the weekly summary", "solve the practice problems", "repair the shelf", "take out the recycling",
]
EVAL_TASKS = [
    "send the invoice", "memorize the formulas", "tidy the bookshelf", "draft the cover letter", "refill the prescription",
    "walk the dog", "compare phone plans", "assemble the report appendix", "write the meeting minutes", "stretch for ten minutes",
    "buy a birthday gift", "check the lab results", "wash the car", "plan the weekend trip", "reply to the survey",
]
SPACES = ["Work", "Study", "Home", "Health", "Side project", "Errands"]
GOALS = ["finish my thesis", "learn Spanish", "get fit for a 10k", "launch my website", "save for a trip", "pass the driving test"]
NOTE_TITLES = ["Productivity pattern", "What my focus data shows", "Focus habits"]


class Gen:
    def __init__(self, split, seed):
        self.split = split
        self.rng = random.Random(seed)
        self.tasks = TRAIN_TASKS if split == "train" else EVAL_TASKS
        self.n = 0

    def pick(self, k):
        return self.rng.sample(self.tasks, k)

    def mins(self):
        return self.rng.choice(MINS)

    def sk(self, domain, request, titles, gold, store=None, spaces=None, goals=None, focus=None, tag=""):
        self.n += 1
        return {
            "sid": f"{self.split[0]}-{domain}-{self.n:03d}",
            "domain": domain,
            "split": self.split,
            "pattern": tag,
            "request_en": request,
            "titles": titles,
            "store": store or [],
            "spaces": spaces or [],
            "goals": goals or [],
            "focus": focus or [],
            "gold": {"actions": gold},
        }

    def stored(self, keys, **over):
        """Saved tasks S1..Sn with their titles."""
        titles = dict(zip(keys, self.pick(len(keys))))
        store = []
        for k in keys:
            store.append({"key": k, "mins": self.mins(), "section": self.rng.choice(["today", "this_week", "backlog"]), "at": None, "deadline": None, "space": None})
        return titles, store


# ---------------------------------------------------------------- domains

def create_task(g):
    r, out = g.rng, []
    t = g.pick(8)
    m = g.mins()
    out.append(g.sk("create_task", f"Add a task to {t[0]}.", {"T1": t[0]}, [{"type": "create_task", "title": "{T1}"}], tag="plain"))
    out.append(g.sk("create_task", f"Please remind me to {t[1]} today.", {"T1": t[1]}, [{"type": "create_task", "title": "{T1}", "section": "today"}], tag="today"))
    out.append(g.sk("create_task", f"New task for this week: {t[2]}.", {"T1": t[2]}, [{"type": "create_task", "title": "{T1}", "section": "this_week"}], tag="week"))
    out.append(g.sk("create_task", f"Someday I should {t[3]}, put it in the backlog.", {"T1": t[3]}, [{"type": "create_task", "title": "{T1}", "section": "backlog"}], tag="backlog"))
    out.append(g.sk("create_task", f"Add {t[4]}, it takes about {m} minutes.", {"T1": t[4]}, [{"type": "create_task", "title": "{T1}", "estimate_mins": m}], tag="minutes"))
    h = r.choice([1, 2, 3])
    out.append(g.sk("create_task", f"Create a task to {t[5]} that needs {h} {'hour' if h == 1 else 'hours'}.", {"T1": t[5]}, [{"type": "create_task", "title": "{T1}", "estimate_mins": 60 * h}], tag="hours"))
    sp = r.choice(SPACES)
    out.append(g.sk("create_task", f"Add {t[6]} to my {sp} space.", {"T1": t[6], "P1": sp}, [{"type": "create_task", "title": "{T1}", "space": "{P1}"}], spaces=["P1"], tag="space"))
    a, b, c = r.sample(g.tasks, 3)
    out.append(g.sk("create_task", f"Make a task called {t[7]} with these steps: {a}, {b} and {c}.", {"T1": t[7], "A": a, "B": b, "C": c}, [{"type": "create_task", "title": "{T1}", "steps": ["{A}", "{B}", "{C}"]}], tag="steps"))
    x, y = r.sample(g.tasks, 2)
    out.append(g.sk("create_task", f"Add two tasks: {x} and {y}.", {"T1": x, "T2": y}, [{"type": "create_task", "title": "{T1}"}, {"type": "create_task", "title": "{T2}"}], tag="two"))
    x, y, z = r.sample(g.tasks, 3)
    out.append(g.sk("create_task", f"I need to {x}, {y} and {z} today.", {"T1": x, "T2": y, "T3": z}, [{"type": "create_task", "title": f"{{T{i}}}", "section": "today"} for i in (1, 2, 3)], tag="three"))
    return out


def due_priority(g):
    r, out = g.rng, []
    t = g.pick(6)
    n = r.choice([2, 3, 4, 5, 7])
    out.append(g.sk("due_priority", f"Add a high priority task to {t[0]} by {{d+{n}}}.", {"T1": t[0]}, [{"type": "create_task", "title": "{T1}", "deadline": f"{{d+{n}}}", "quadrant": "do_asap"}], tag="high-iso"))
    out.append(g.sk("due_priority", f"Add a low priority task to {t[1]}, due the day after tomorrow.", {"T1": t[1]}, [{"type": "create_task", "title": "{T1}", "deadline": "{d+2}", "quadrant": "leave_till_end"}], tag="low"))
    out.append(g.sk("due_priority", f"New urgent task: {t[2]}, due tomorrow.", {"T1": t[2]}, [{"type": "create_task", "title": "{T1}", "deadline": "{d+1}", "quadrant": "do_asap"}], tag="urgent"))
    out.append(g.sk("due_priority", f"Add {t[3]} with medium priority, due in a week.", {"T1": t[3]}, [{"type": "create_task", "title": "{T1}", "deadline": "{d+7}", "quadrant": "do_mid_week"}], tag="medium"))
    out.append(g.sk("due_priority", f"I must {t[4]} by {{d+{n + 3}}}, it is important.", {"T1": t[4]}, [{"type": "create_task", "title": "{T1}", "deadline": f"{{d+{n + 3}}}", "quadrant": "do_mid_week"}], tag="important"))
    titles, store = g.stored(["S1", "S2"])
    k = r.choice([3, 4, 6])
    out.append(g.sk("due_priority", f"Set the deadline of {titles['S1']} to {{d+{k}}}.", titles, [{"type": "update_task", "task": "@S1", "deadline": f"{{d+{k}}}"}], store=store, tag="update-deadline"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("due_priority", f"Make {titles['S2']} high priority.", titles, [{"type": "set_quadrant", "task": "@S2", "quadrant": "do_asap"}], store=store, tag="set-priority"))
    return out


def start_focus(g):
    r, out = g.rng, []
    titles, store = g.stored(["S1", "S2", "S3"])
    m = r.choice([25, 30, 45, 50])
    out.append(g.sk("start_focus", f"Start a {m} minute focus session on {titles['S1']}.", titles, [{"type": "start_focus", "task": "@S1"}], store=store, tag="existing"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("start_focus", f"Let's begin working on {titles['S2']} now.", titles, [{"type": "start_focus", "task": "@S2"}], store=store, tag="begin"))
    titles, store = g.stored(["S1", "S2"])
    new = r.choice([x for x in g.tasks if x not in titles.values()])
    titles["T1"] = new
    out.append(g.sk("start_focus", f"Start the timer to {new}.", titles, [{"type": "start_focus", "task": "{T1}"}], store=store, tag="new-title"))
    out.append(g.sk("start_focus", "Stop the timer.", {}, [{"type": "stop_focus"}], tag="stop"))
    titles, store = g.stored(["S1", "S2", "S3"])
    out.append(g.sk("start_focus", f"Pomodoro on {titles['S3']}, please.", titles, [{"type": "start_focus", "task": "@S3"}], store=store, tag="pomodoro"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("start_focus", "I am done for now, stop focusing.", titles, [{"type": "stop_focus"}], store=store, tag="stop-done"))
    return out


def log_time(g):
    r, out = g.rng, []
    titles, store = g.stored(["S1", "S2", "S3"])
    m = r.choice([20, 30, 45, 50, 75])
    out.append(g.sk("log_time", f"Log {m} minutes on {titles['S1']}.", titles, [{"type": "log_time", "task": "@S1", "minutes": m}], store=store, tag="minutes"))
    titles, store = g.stored(["S1", "S2"])
    h = r.choice([1, 2, 3])
    out.append(g.sk("log_time", f"I worked {h} {'hour' if h == 1 else 'hours'} on {titles['S2']} yesterday.", titles, [{"type": "log_time", "task": "@S2", "minutes": 60 * h, "date": "{d-1}"}], store=store, tag="yesterday"))
    titles, store = g.stored(["S1", "S2"])
    new = r.choice([x for x in g.tasks if x not in titles.values()])
    titles["T1"] = new
    m = r.choice([30, 40, 90])
    out.append(g.sk("log_time", f"Record {m} minutes spent to {new}.", titles, [{"type": "create_task", "title": "{T1}"}, {"type": "log_time", "task": "{T1}", "minutes": m}], store=store, tag="new-then-log"))
    titles, store = g.stored(["S1", "S2", "S3"])
    out.append(g.sk("log_time", f"Add 1 hour 30 minutes to {titles['S3']} for today.", titles, [{"type": "log_time", "task": "@S3", "minutes": 90}], store=store, tag="mixed"))
    titles, store = g.stored(["S1", "S2"])
    n = r.choice([2, 3])
    out.append(g.sk("log_time", f"Log 45 minutes on {titles['S1']} from {n} days ago.", titles, [{"type": "log_time", "task": "@S1", "minutes": 45, "date": f"{{d-{n}}}"}], store=store, tag="days-ago"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("log_time", f"I spent 25 minutes on {titles['S1']} and 40 minutes on {titles['S2']}.", titles, [{"type": "log_time", "task": "@S1", "minutes": 25}, {"type": "log_time", "task": "@S2", "minutes": 40}], store=store, tag="two"))
    return out


def reschedule(g):
    r, out = g.rng, []

    def clash():
        titles, store = g.stored(["S1", "S2", "S3"])
        for s in store[:2]:
            s["at"] = "{d+1@10:00}"
            s["mins"] = 60
            s["section"] = "this_week"
        return titles, store

    titles, store = clash()
    out.append(g.sk("reschedule", "Two of my tasks are at the same time tomorrow. Move one so they do not clash.", titles, [{"type": "schedule_task", "task": "@S2", "at": "{d+1@11:00}"}], store=store, tag="clash"))
    titles, store = clash()
    out.append(g.sk("reschedule", f"Move {titles['S1']} to tomorrow at 14:00.", titles, [{"type": "schedule_task", "task": "@S1", "at": "{d+1@14:00}"}], store=store, tag="move-time"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("reschedule", f"Push {titles['S1']} to next week.", titles, [{"type": "update_task", "task": "@S1", "do_date": "{d+7}"}], store=store, tag="next-week"))
    titles, store = g.stored(["S1", "S2", "S3"])
    for s in store:
        s["section"] = "today"
    out.append(g.sk("reschedule", "I am sick, move everything from today to tomorrow.", titles, [{"type": "update_task", "task": f"@S{i}", "do_date": "{d+1}"} for i in (1, 2, 3)], store=store, tag="all-today"))
    titles, store = clash()
    out.append(g.sk("reschedule", f"Schedule {titles['S3']} on {{d+2}} at 09:30.", titles, [{"type": "schedule_task", "task": "@S3", "at": "{d+2@09:30}"}], store=store, tag="schedule-iso"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("reschedule", f"Move {titles['S2']} to the backlog.", titles, [{"type": "update_task", "task": "@S2", "section": "backlog"}], store=store, tag="to-backlog"))
    titles, store = g.stored(["S1", "S2"])
    store[0]["section"] = "backlog"
    out.append(g.sk("reschedule", f"Do {titles['S1']} today after all.", titles, [{"type": "update_task", "task": "@S1", "section": "today"}], store=store, tag="to-today"))
    return out


def patterns(g):
    r, out = g.rng, []
    for i in range(5):
        parts = {"morning": r.choice([40, 80, 120, 200]), "afternoon": r.choice([60, 150, 260, 340]), "evening": r.choice([0, 30, 60, 90])}
        best = max(parts, key=parts.get)
        day = r.choice(["Mon", "Tue", "Wed", "Thu", "Fri"])
        sess = r.choice([20, 25, 30, 40, 50])
        # Build focus entries that add up to the stated minutes, in 'sess' minute sessions.
        focus, days = [], list(range(1, 13))
        hours = {"morning": 9, "afternoon": 14, "evening": 19}
        for part, total in parts.items():
            left = total
            while left > 0:
                use = min(sess, left)
                focus.append({"days_ago": r.choice(days), "hour": hours[part], "mins": use})
                left -= use
        title = r.choice(NOTE_TITLES)
        body = f"You focus most in the {best} ({parts[best]} minutes over 14 days). Average session: {sess} minutes. Plan your hardest work in the {best}."
        ask = ["When am I most productive?", "Look at my focus history and tell me my best time of day.", "What do my focus habits look like?", "Which part of the day should I do deep work?", "Analyse my productivity pattern."][i]
        out.append(g.sk("patterns", ask, {"N1": title, "B1": body}, [{"type": "create_note", "title": "{N1}", "body": "{B1}"}], focus=focus, tag="best-time"))
    return out


def plan(g):
    r, out = g.rng, []
    for i in range(5):
        weeks = [2, 3, 4, 4, 3][i]
        goal = r.choice(GOALS)
        topic = g.pick(1)[0]
        titles = {"G1": goal}
        ms = []
        tasks = []
        for w in range(1, weeks + 1):
            titles[f"M{w}"] = f"Week {w}: part {w} of {goal}"
            ms.append({"title": f"{{M{w}}}", "date": f"{{d+{7 * w}}}"})
            for k in (1, 2):
                titles[f"K{w}{k}"] = f"{r.choice(g.tasks)} (week {w})"
                tasks.append({"title": f"{{K{w}{k}}}", "estimate_mins": r.choice([45, 60, 90]), "milestone": f"{{M{w}}}"})
        req = f"Plan a {weeks} week schedule to {goal}, finishing by {{d+{7 * weeks}}}."
        out.append(g.sk("plan", req, titles, [{"type": "create_goal", "title": "{G1}", "priority": "high", "target_date": f"{{d+{7 * weeks}}}", "milestones": ms, "tasks": tasks}], tag=f"{weeks}-weeks"))
    return out


def manage(g):
    r, out = g.rng, []
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("manage", f"I finished {titles['S1']}.", titles, [{"type": "complete_task", "task": "@S1"}], store=store, tag="complete"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("manage", f"Delete the task {titles['S2']}.", titles, [{"type": "delete_task", "task": "@S2"}], store=store, tag="delete"))
    titles, store = g.stored(["S1", "S2"])
    c = r.choice(g.tasks)
    titles["T1"] = c
    out.append(g.sk("manage", f"Add a step '{c}' under {titles['S1']}.", titles, [{"type": "add_subtask", "task": "@S1", "title": "{T1}"}], store=store, tag="subtask"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("manage", f"Change {titles['S1']} to 45 minutes.", titles, [{"type": "update_task", "task": "@S1", "estimate_mins": 45}], store=store, tag="estimate"))
    sp = r.choice(SPACES)
    out.append(g.sk("manage", f"Create a new space called {sp}.", {"P1": sp}, [{"type": "create_space", "name": "{P1}"}], tag="create-space"))
    sp1, sp2 = r.sample(SPACES, 2)
    out.append(g.sk("manage", f"Rename my {sp1} space to {sp2}.", {"P1": sp1, "P2": sp2}, [{"type": "rename_space", "space": "{P1}", "name": "{P2}"}], spaces=["P1"], tag="rename-space"))
    sp = r.choice(SPACES)
    out.append(g.sk("manage", f"Delete the space {sp}.", {"P1": sp}, [{"type": "delete_space", "space": "{P1}"}], spaces=["P1"], tag="delete-space"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("manage", f"Mark {titles['S2']} as not done, I need to redo it.", titles, [{"type": "complete_task", "task": "@S2", "done": False}], store=store, tag="reopen"))
    note = r.choice(["ideas for the weekend", "things to ask the doctor", "books to read this year"])
    out.append(g.sk("manage", f"Create a note titled {note}.", {"N1": note}, [{"type": "create_note", "title": "{N1}"}], tag="note"))
    titles, store = g.stored(["S1", "S2"])
    gl = r.choice(GOALS)
    titles["G1"] = gl
    out.append(g.sk("manage", f"Link {titles['S1']} to my goal {gl}.", titles, [{"type": "link_task", "goal": "{G1}", "task": "@S1"}], store=store, goals=["G1"], tag="link"))
    return out


def guard(g):
    r, out = g.rng, []
    for text in ["Move it to later.", "Do the thing we talked about.", "Change that task."]:
        titles, store = g.stored(["S1", "S2"])
        out.append(g.sk("guard", text, titles, [], store=store, tag="unclear"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("guard", f"Add a task to {titles['S1']}.", titles, [], store=store, tag="duplicate"))
    for text in ["What is the capital of France?", "Explain the difference between SRAM and DRAM.", "Tell me a joke.", "How do I stay motivated?"]:
        titles, store = g.stored(["S1", "S2"])
        out.append(g.sk("guard", text, titles, [], store=store, tag="chat"))
    return out


def gaps(g):
    """Patterns round 3 showed the model missing in every language: the day to
    work on something (do_date) versus the day it is due (deadline), counting
    days, past-day time on a new task, notes with contents and small talk.
    Appended last so every older scenario keeps its id and translation."""
    r, out = g.rng, []
    t = g.pick(6)
    n = r.choice([2, 3, 4, 5, 6])
    out.append(g.sk("due_priority", f"Medium priority: {t[0]}, to do the day after tomorrow.", {"T1": t[0]}, [{"type": "create_task", "title": "{T1}", "do_date": "{d+2}", "quadrant": "do_mid_week"}], tag="do-date-medium"))
    out.append(g.sk("due_priority", f"High priority, but I will {t[1]} in {n} days.", {"T1": t[1]}, [{"type": "create_task", "title": "{T1}", "do_date": f"{{d+{n}}}", "quadrant": "do_asap"}], tag="do-date-high"))
    out.append(g.sk("due_priority", f"Add {t[2]} for tomorrow, low priority.", {"T1": t[2]}, [{"type": "create_task", "title": "{T1}", "do_date": "{d+1}", "quadrant": "leave_till_end"}], tag="do-date-low"))
    out.append(g.sk("due_priority", f"{t[3]} is due in {n + 1} days, I will start it tomorrow.", {"T1": t[3]}, [{"type": "create_task", "title": "{T1}", "do_date": "{d+1}", "deadline": f"{{d+{n + 1}}}"}], tag="do-and-due"))
    titles, store = g.stored(["S1", "S2"])
    k = r.choice([2, 3, 4, 5])
    out.append(g.sk("reschedule", f"Change {titles['S1']} to {k} days from now.", titles, [{"type": "update_task", "task": "@S1", "do_date": f"{{d+{k}}}"}], store=store, tag="in-n-days"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("reschedule", f"Move {titles['S2']} to the day after tomorrow.", titles, [{"type": "update_task", "task": "@S2", "do_date": "{d+2}"}], store=store, tag="after-tomorrow"))
    titles, store = g.stored(["S1", "S2"])
    hh = r.choice(["09:00", "10:00", "16:30"])
    out.append(g.sk("reschedule", f"The day after tomorrow at {hh}, {titles['S1']}.", titles, [{"type": "schedule_task", "task": "@S1", "at": f"{{d+2@{hh}}}"}], store=store, tag="after-tomorrow-time"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("reschedule", f"Plan {titles['S2']} for {n + 1} days from now at 11:00.", titles, [{"type": "schedule_task", "task": "@S2", "at": f"{{d+{n + 1}@11:00}}"}], store=store, tag="in-n-days-time"))
    titles, store = g.stored(["S1"])
    new = r.choice([x for x in g.tasks if x not in titles.values()])
    titles["T1"] = new
    m = r.choice([25, 50, 80])
    out.append(g.sk("log_time", f"Yesterday I spent {m} minutes to {new}.", titles, [{"type": "create_task", "title": "{T1}"}, {"type": "log_time", "task": "{T1}", "minutes": m, "date": "{d-1}"}], store=store, tag="new-yesterday"))
    titles, store = g.stored(["S1"])
    new = r.choice([x for x in g.tasks if x not in titles.values()])
    titles["T1"] = new
    out.append(g.sk("log_time", f"Two days ago I worked 2 hours to {new}, log it.", titles, [{"type": "create_task", "title": "{T1}"}, {"type": "log_time", "task": "{T1}", "minutes": 120, "date": "{d-2}"}], store=store, tag="new-days-ago"))
    titles, store = g.stored(["S1", "S2"])
    out.append(g.sk("log_time", f"{titles['S2']} took me 35 minutes yesterday.", titles, [{"type": "log_time", "task": "@S2", "minutes": 35, "date": "{d-1}"}], store=store, tag="took-yesterday"))
    note, body = r.choice([("shopping list", "rice, sugar and oil"), ("questions for the doctor", "ask about the headaches and the new tablets"), ("gift ideas", "a book, a scarf and a plant")])
    out.append(g.sk("manage", f"Make a note called {note} with: {body}.", {"N1": note, "B1": body}, [{"type": "create_note", "title": "{N1}", "body": "{B1}"}], tag="note-body"))
    for text in ["Hi, how are you today?", "What is the weather like?", "Good morning!", "Thanks, that is all for now.", "Who won the football match yesterday?"]:
        titles, store = g.stored(["S1", "S2"])
        out.append(g.sk("guard", text, titles, [], store=store, tag="small-talk"))
    return out


DOMAINS = [create_task, due_priority, start_focus, log_time, reschedule, patterns, plan, manage, guard, gaps]


def build():
    skeletons = []
    for split, seed in (("train", 11), ("eval", 97)):
        g = Gen(split, seed)
        for d in DOMAINS:
            skeletons.extend(d(g))
        # More train variety: run each domain again with new random picks.
        if split == "train":
            for extra in range(1, 2):
                g2 = Gen("train", seed + extra)
                g2.n = 1000 * extra
                for d in DOMAINS:
                    skeletons.extend(d(g2))
    return skeletons


if __name__ == "__main__":
    out_dir = sys.argv[1]
    sks = build()
    json.dump(sks, open(out_dir + "/skeletons.json", "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    from collections import Counter
    print(Counter((s["split"], s["domain"]) for s in sks))
    print(len(sks), "skeletons")
