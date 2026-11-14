"""Kaggle job: LoRA fine-tune the Thulli planner, then test every language and task.

Inputs (Kaggle datasets):  /kaggle/input/thulli-finetune-data/rows.jsonl
Outputs (/kaggle/working): results.jsonl, summary.json, merged/ (fp16 model)

Environment switches:
  SMOKE=1            tiny run to prove the pipeline
  TRAIN_PER_LANG=40  training rows per language (default 40)
  EVAL_PER_CELL=2    test rows per language and task (default 2; 0 = all)
  EPOCHS=1  LR=2e-4  BASE=<hf id or path>  RESUME=<adapter dir>
"""
import collections
import json
import math
import os
import random
import subprocess
import sys
import time
from pathlib import Path

SMOKE = os.environ.get("SMOKE") == "1"
BASE = os.environ.get("BASE", "DavidAU/Qwen3.5-2B-Polaris-HighIQ-Thinking-Compact")
TRAIN_PER_LANG = int(os.environ.get("TRAIN_PER_LANG", "40"))
EVAL_PER_CELL = int(os.environ.get("EVAL_PER_CELL", "2"))
EPOCHS = float(os.environ.get("EPOCHS", "1"))
LR = float(os.environ.get("LR", "2e-4"))
MAX_LEN = 3600
MAX_NEW = int(os.environ.get("MAX_NEW", "900"))
GEN_BATCH = int(os.environ.get("GEN_BATCH", "12"))
ACCUM = 16
_found = sorted(Path("/kaggle/input").rglob("rows.jsonl*"), key=lambda q: (not str(q).endswith(".gz"), str(q))) if Path("/kaggle/input").exists() else []
DATA = Path(os.environ.get("DATA") or (str(_found[0]) if _found else "rows.jsonl"))
OUT = Path(os.environ.get("OUT", "/kaggle/working"))
RESUME = os.environ.get("RESUME")
if RESUME == "auto":
    _ad = sorted(Path("/kaggle/input").rglob("adapter_config.json"))
    RESUME = str(_ad[0].parent) if _ad else None
    print("resuming from", RESUME, flush=True)
SEED = int(os.environ.get("SEED", "7"))
# Tasks to see more often per pass (comma separated), and how many times.
FOCUS = [d for d in os.environ.get("FOCUS", "").split(",") if d]
FOCUS_X = int(os.environ.get("FOCUS_X", "2"))


def sh(cmd):
    print("$", cmd, flush=True)
    subprocess.run(cmd, shell=True, check=False)


if os.environ.get("_THULLI_WORKER") != "1":
    sh(f"{sys.executable} -m pip install -q -U 'transformers>=5.2' peft accelerate flash-linear-attention")
    sh(f"{sys.executable} -m pip uninstall -y -q torchao")
    os.environ["_THULLI_WORKER"] = "1"

import torch  # noqa: E402
from transformers import AutoModelForCausalLM, AutoTokenizer  # noqa: E402



# ---------------------------------------------------------------- data

def load_rows():
    import gzip
    opener = gzip.open if str(DATA).endswith(".gz") else open
    rows = [json.loads(l) for l in opener(DATA, "rt", encoding="utf-8") if l.strip()]
    train = [r for r in rows if r["split"] == "train"]
    test = [r for r in rows if r["split"] == "eval"]
    by_lang = collections.defaultdict(list)
    for r in train:
        by_lang[r["lang"]].append(r)
    picked = []
    per = 3 if SMOKE else TRAIN_PER_LANG
    langs = sorted(by_lang)[:3] if SMOKE else sorted(by_lang)
    for lang in langs:
        rs = by_lang[lang]
        random.shuffle(rs)
        if per == 0:
            picked.extend(rs)
            continue
        # Spread across tasks: round-robin by domain.
        dom = collections.defaultdict(list)
        for r in rs:
            dom[r["domain"]].append(r)
        order = []
        while len(order) < per and any(dom.values()):
            for d in sorted(dom):
                for _ in range(FOCUS_X if d in FOCUS else 1):
                    if dom[d] and len(order) < per:
                        order.append(dom[d].pop())
        picked.extend(order)
    random.shuffle(picked)
    cell = collections.defaultdict(list)
    for r in test:
        cell[(r.get("suite", "templated"), r["lang"], r["domain"])].append(r)
    evals = []
    for (suite, lang, dom), rs in sorted(cell.items()):
        if SMOKE and lang not in langs:
            continue
        # Only the translated frames are thinned out; the other suites are tested whole.
        limit = EVAL_PER_CELL if suite == "templated" else 0
        evals.extend(rs if limit == 0 else rs[:limit])
    if SMOKE:
        evals = evals[:12]
    return picked, evals


def prompt_text(r):
    return f"<|im_start|>system\n{r['system']}<|im_end|>\n<|im_start|>user\n{r['user']}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"


# ---------------------------------------------------------------- scoring

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
    "create_goal": {"type", "title", "description", "category", "priority", "target_date", "milestones", "tasks"},
    "link_task": {"type", "goal", "task", "milestone"},
}


def parse_reply(text):
    text = text.split("</think>")[-1].strip()
    i = text.find("{")
    if i < 0:
        return None
    try:
        obj, _ = json.JSONDecoder().raw_decode(text[i:])
    except Exception:
        return None
    acts = obj.get("actions") if isinstance(obj, dict) else None
    return acts if isinstance(acts, list) else None


def norm(a):
    out = {}
    for k, v in a.items():
        if v in (None, "", [], 0) and k != "done":
            continue
        if k == "section" and v == "today":
            continue
        if isinstance(v, str):
            v = v.strip().casefold()
        if isinstance(v, list):
            v = [x.strip().casefold() if isinstance(x, str) else x for x in v]
        out[k] = v
    return json.dumps(out, sort_keys=True, ensure_ascii=False)


def numbers(s):
    import re
    return set(re.findall(r"\d+", s))


def tags_of(text):
    import re
    return [int(x) for x in re.findall(r"\[(\d+)\]", text)]


def invented(found, source):
    """Numbers of 3+ digits (years, amounts) the source does not hold. Small counters may be written as digits."""
    return {n for n in found - numbers(source) if len(n) > 2}


def score_research(row, reply):
    """Research steps: right shape, nothing invented. Wording is not compared."""
    import re
    d, text = row["domain"], reply.split("</think>")[-1].strip()
    if d in ("research_plan", "research_gap"):
        i = text.find("{")
        try:
            obj, _ = json.JSONDecoder().raw_decode(text[i:]) if i >= 0 else (None, 0)
        except Exception:
            obj = None
        qs = obj.get("queries") if isinstance(obj, dict) else None
        if not isinstance(qs, list) or not all(isinstance(q, str) and q.strip() for q in qs):
            return False, "not valid json"
        gold = json.loads(row["target"])["queries"]
        if d == "research_plan":
            return (2 <= len(qs) <= 4), "query count"
        if gold and not qs:
            return False, "no new search although something is missing"
        return len(qs) <= 3, "too many searches"
    if d == "research_extract":
        if row["target"].strip() == "NONE":
            return text == "NONE", "should be NONE"
        if text == "NONE" or not text:
            return False, "dropped a useful part"
        need, have = numbers(row["target"]), numbers(text)
        if not need <= have:
            return False, "numbers missing"
        if invented(have, row["user"]):
            return False, "number invented"
        return True, ""
    if d == "research_merge":
        if not set(tags_of(row["target"])) <= set(tags_of(text)):
            return False, "tag lost"
        if not numbers(row["target"]) <= numbers(text):
            return False, "numbers missing"
        if invented(numbers(text), row["user"]):
            return False, "number invented"
        return True, ""
    if d == "research_answer":
        have_tags = set(tags_of(row["user"].split("Notes:")[-1]))
        used = tags_of(text)
        if not used or not set(used) <= have_tags:
            return False, "tags missing or invented"
        # An answer may leave a detail out; it must keep most of the key numbers,
        # invent none and not fall into a loop.
        need = numbers(row["target"])
        if need and len(need & numbers(text)) * 2 < len(need):
            return False, "numbers missing"
        if invented(numbers(text), row["user"]):
            return False, "number invented"
        if looping(text):
            return False, "repeats itself"
        return True, ""
    return False, "unknown step"


def looping(text):
    """True when one 4-word run repeats 6 or more times: the reply went into a loop."""
    w = text.split()
    runs = collections.Counter(" ".join(w[i : i + 4]) for i in range(max(0, len(w) - 3)))
    return any(n >= 6 for n in runs.values())


def score(row, reply):
    """(passed, reason). Strict on shape and key values."""
    if row["domain"].startswith("research_"):
        return score_research(row, reply)
    acts = parse_reply(reply)
    if acts is None:
        return False, "not valid json"
    for a in acts:
        if not isinstance(a, dict) or a.get("type") not in ALLOWED:
            return False, "unknown action type"
        if set(a) - ALLOWED[a["type"]]:
            return False, "extra keys"
    gold = row["gold"]["actions"]
    d = row["domain"]
    if d == "guard":
        return (not acts), "should not act"
    if not acts:
        return False, "no actions"
    if d == "plan":
        g, p = gold[0], acts[0]
        if len(acts) != 1 or p["type"] != "create_goal":
            return False, "not one goal"
        if p.get("target_date") != g["target_date"]:
            return False, "wrong target date"
        ms = [m["title"] if isinstance(m, dict) else m for m in p.get("milestones", [])]
        if len(ms) != len(g["milestones"]):
            return False, "milestone count"
        tasks = p.get("tasks", [])
        if len(tasks) < len(ms) or any((t.get("milestone") if isinstance(t, dict) else None) not in ms for t in tasks):
            return False, "tasks not pinned to milestones"
        return True, ""
    if d == "patterns":
        if len(acts) != 1 or acts[0]["type"] != "create_note":
            return False, "not one note"
        need = numbers(row["gold"]["actions"][0].get("body", ""))
        return (need <= numbers(acts[0].get("body", "")) and len(need) > 0), "numbers missing"
    if row.get("suite") == "massive":
        # Real requests: one new task whose title is the event named in them.
        # The labelled event name is often a fragment ("conference" in "the conference movie"),
        # so a title that holds it, or sits inside it, counts.
        got = " ".join(str(acts[0].get("title", "")).split()).casefold() if acts else ""
        want = " ".join(gold[0]["title"].split()).casefold()
        ok = len(acts) == 1 and acts[0]["type"] == "create_task" and bool(got) and (want in got or got in want)
        return ok, "wrong title or action"
    if sorted(norm(a) for a in acts) != sorted(norm(a) for a in gold):
        return False, "differs from expected"
    return True, ""


# ---------------------------------------------------------------- model

def worker(rank, world):
    import torch.distributed as dist
    from peft import LoraConfig, PeftModel, get_peft_model

    random.seed(SEED)
    torch.manual_seed(SEED)
    torch.cuda.set_device(rank)
    dev = f"cuda:{rank}"
    if world > 1:
        dist.init_process_group("nccl", init_method="tcp://127.0.0.1:29511", rank=rank, world_size=world)
    log = (lambda *a: print(*a, flush=True)) if rank == 0 else (lambda *a: None)
    log("gpus:", world, [torch.cuda.get_device_name(i) for i in range(world)])

    tok = AutoTokenizer.from_pretrained(BASE)
    model = AutoModelForCausalLM.from_pretrained(BASE, dtype=torch.float16, device_map={"": rank})
    log(type(model).__name__, "params", sum(p.numel() for p in model.parameters()) / 1e9, "B")
    if RESUME:
        model = PeftModel.from_pretrained(model, RESUME, is_trainable=True)
    else:
        model = get_peft_model(model, LoraConfig(r=32, lora_alpha=64, lora_dropout=0.05, task_type="CAUSAL_LM", target_modules="all-linear"))
    for n, p in model.named_parameters():
        if p.requires_grad:
            p.data = p.data.float()
    if rank == 0:
        model.print_trainable_parameters()
    model.gradient_checkpointing_enable(gradient_checkpointing_kwargs={"use_reentrant": False})
    model.enable_input_require_grads()
    model.config.use_cache = False

    train_rows, eval_rows = load_rows()
    log(len(train_rows), "train rows,", len(eval_rows), "eval rows")

    # ---- train
    enc = []
    for r in train_rows:
        pp = tok(prompt_text(r), add_special_tokens=False).input_ids
        tt = tok(r["target"] + "<|im_end|>", add_special_tokens=False).input_ids
        if len(pp) + len(tt) <= MAX_LEN:
            enc.append((pp, tt))
    log(len(enc), "usable;", sum(len(a) + len(b) for a, b in enc) / 1e6, "M tokens")

    params = [p for p in model.parameters() if p.requires_grad]
    opt = torch.optim.AdamW(params, lr=LR, weight_decay=0.0, betas=(0.9, 0.99))
    steps = max(1, int(len(enc) * EPOCHS) // ACCUM)
    scaler = torch.amp.GradScaler("cuda")
    model.train()
    order = []
    rng = random.Random(SEED)
    while len(order) < steps * ACCUM:
        idx = list(range(len(enc)))
        rng.shuffle(idx)
        order.extend(idx)
    t0, seen = time.time(), 0
    for step in range(steps):
        lr = LR * min(1.0, (step + 1) / 10) * (0.5 * (1 + math.cos(math.pi * step / steps)) * 0.9 + 0.1)
        for g in opt.param_groups:
            g["lr"] = lr
        tot = 0.0
        for k in range(rank, ACCUM, world):
            pp, tt = enc[order[step * ACCUM + k]]
            ids = torch.tensor([pp + tt], device=dev)
            keep = torch.arange(len(pp) - 1, len(pp) + len(tt) - 1, device=dev)
            with torch.autocast("cuda", dtype=torch.float16):
                out = model(input_ids=ids, logits_to_keep=keep)
            loss = torch.nn.functional.cross_entropy(out.logits[0].float(), torch.tensor(tt, device=dev)) / ACCUM
            scaler.scale(loss).backward()
            tot += loss.item()
            seen += len(pp) + len(tt)
        if world > 1:
            # Every rank sees the same summed gradients, so the loss scaler stays in step.
            for p in params:
                if p.grad is None:
                    p.grad = torch.zeros_like(p)
                dist.all_reduce(p.grad)
        scaler.unscale_(opt)
        torch.nn.utils.clip_grad_norm_(params, 1.0)
        scaler.step(opt)
        scaler.update()
        opt.zero_grad(set_to_none=True)
        if SMOKE or step % 5 == 0 or step == steps - 1:
            log(f"step {step + 1}/{steps} loss {tot * world:.4f} lr {lr:.2e} {seen * world / (time.time() - t0):.0f} tok/s total, elapsed {time.time() - t0:.0f}s")

    if rank == 0:
        model.save_pretrained(OUT / "adapter")
    log("merging")
    model = model.merge_and_unload()
    model.config.use_cache = True
    model.eval()
    if rank == 0 and not SMOKE:
        model.save_pretrained(OUT / "merged", safe_serialization=True)
        tok.save_pretrained(OUT / "merged")

    # ---- test: each GPU takes every world-th row
    tok.padding_side = "left"
    im_end = tok.convert_tokens_to_ids("<|im_end|>")
    eval_rows.sort(key=lambda r: len(r["system"]) + len(r["user"]))
    mine = eval_rows[rank::world]
    results, t0 = [], time.time()
    for i in range(0, len(mine), GEN_BATCH):
        batch = mine[i : i + GEN_BATCH]
        eb = tok([prompt_text(r) for r in batch], return_tensors="pt", padding=True, add_special_tokens=False).to(dev)
        with torch.no_grad():
            gen = model.generate(**eb, max_new_tokens=MAX_NEW, do_sample=False, eos_token_id=im_end, pad_token_id=tok.pad_token_id)
        for r, g in zip(batch, gen):
            new = g[eb.input_ids.shape[1]:]
            text = tok.decode(new, skip_special_tokens=True)
            ok, why = score(r, text)
            results.append({"id": r["id"], "lang": r["lang"], "suite": r.get("suite", "templated"), "domain": r["domain"], "pass": ok, "why": "" if ok else why, "reply": text, "target": r["target"], "tokens": int((new != tok.pad_token_id).sum())})
        log(f"tested {len(results) * world}/{len(eval_rows)} pass so far {sum(x['pass'] for x in results) / len(results):.1%} ({time.time() - t0:.0f}s)")
    with open(OUT / f"results.part{rank}.jsonl", "w", encoding="utf-8") as f:
        for r in results:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    if world > 1:
        dist.barrier()
    if rank == 0:
        allr = []
        for i in range(world):
            allr += [json.loads(l) for l in open(OUT / f"results.part{i}.jsonl", encoding="utf-8")]
            os.remove(OUT / f"results.part{i}.jsonl")
        with open(OUT / "results.jsonl", "w", encoding="utf-8") as f:
            for r in allr:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        by_dom, by_lang, by_suite = collections.defaultdict(list), collections.defaultdict(list), collections.defaultdict(list)
        by_suite_lang = collections.defaultdict(lambda: collections.defaultdict(list))
        for r in allr:
            by_dom[(r["suite"], r["domain"])].append(r["pass"])
            by_lang[r["lang"]].append(r["pass"])
            by_suite[r["suite"]].append(r["pass"])
            by_suite_lang[r["suite"]][r["lang"]].append(r["pass"])
        summary = {
            "rows_tested": len(allr),
            "pass_rate": sum(r["pass"] for r in allr) / max(1, len(allr)),
            "by_suite": {s: {"rows": len(v), "pass": round(sum(v) / len(v), 4)} for s, v in sorted(by_suite.items())},
            "by_domain": {f"{s}/{d}": round(sum(v) / len(v), 4) for (s, d), v in sorted(by_dom.items())},
            "languages_below_95_by_suite": {
                s: {l: round(sum(v) / len(v), 3) for l, v in sorted(langs.items()) if sum(v) / len(v) < 0.95}
                for s, langs in sorted(by_suite_lang.items())
            },
            "languages_below_100": {l: round(sum(v) / len(v), 3) for l, v in sorted(by_lang.items()) if sum(v) != len(v)},
            "max_tokens": max((r["tokens"] for r in allr), default=0),
        }
        json.dump(summary, open(OUT / "summary.json", "w"), indent=1)
        print(json.dumps(summary, indent=1)[:3000], flush=True)
    if world > 1:
        dist.destroy_process_group()


if __name__ == "__main__":
    import torch.multiprocessing as mp

    n = torch.cuda.device_count()
    print("cuda devices:", n, flush=True)
    if n > 1:
        mp.spawn(worker, args=(n,), nprocs=n)
    else:
        worker(0, 1)
