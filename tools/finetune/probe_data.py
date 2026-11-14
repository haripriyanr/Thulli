"""Kaggle job (CPU): look inside MASSIVE and Tatoeba and report what is there.
Nothing is downloaded to the developer's computer; this runs on Kaggle."""
import collections
import json
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

W = Path("/kaggle/working")
W.mkdir(exist_ok=True, parents=True)
out = {}


def fetch(url, dest):
    print("downloading", url, flush=True)
    urllib.request.urlretrieve(url, dest)
    print("  got", Path(dest).stat().st_size, "bytes", flush=True)


# ---- MASSIVE
fetch("https://amazon-massive-nlu-dataset.s3.amazonaws.com/amazon-massive-dataset-1.1.tar.gz", "/tmp/massive.tar.gz")
with tarfile.open("/tmp/massive.tar.gz") as t:
    t.extractall("/tmp/massive")
files = sorted(Path("/tmp/massive").rglob("*.jsonl"))
out["massive_locales"] = [f.stem for f in files]
rows = [json.loads(l) for l in open(next(f for f in files if f.stem == "en-US"), encoding="utf-8")]
out["massive_row_keys"] = sorted(rows[0].keys())
sc = collections.Counter((r["scenario"], r["intent"]) for r in rows)
out["massive_intents_en"] = {f"{a}_{b}": n for (a, b), n in sorted(sc.items())}
keep = [r for r in rows if r["scenario"] in ("lists", "calendar", "alarm", "datetime", "general", "qa")][:0]
samples = {}
for loc in ("en-US", "hi-IN", "ta-IN", "ja-JP", "sw-KE", "am-ET"):
    f = next((x for x in files if x.stem == loc), None)
    if not f:
        continue
    rs = [json.loads(l) for l in open(f, encoding="utf-8")]
    samples[loc] = {}
    for key in ("lists_createoradd", "lists_remove", "calendar_set", "calendar_remove", "alarm_set"):
        sc_, it = key.split("_", 1)
        hit = [r for r in rs if r["scenario"] == sc_ and r["intent"] == it][:3]
        samples[loc][key] = [{"utt": r["utt"], "annot_utt": r["annot_utt"]} for r in hit]
out["massive_samples"] = samples
slots = collections.Counter()
for r in rows:
    if r["scenario"] in ("lists", "calendar"):
        import re
        for m in re.finditer(r"\[(\w+) : ", r["annot_utt"]):
            slots[(r["scenario"], r["intent"], m.group(1))] += 1
out["massive_slots_lists_calendar"] = {"|".join(k): v for k, v in sorted(slots.items())}

# ---- Tatoeba
subprocess.run([sys.executable, "-m", "pip", "install", "-q", "pycountry"], check=False)
fetch("https://downloads.tatoeba.org/exports/sentences.tar.bz2", "/tmp/sentences.tar.bz2")
with tarfile.open("/tmp/sentences.tar.bz2") as t:
    t.extractall("/tmp/tatoeba")
ids = json.loads(Path("/kaggle/input").rglob("languages.json").__next__().read_text(encoding="utf-8")) if list(Path("/kaggle/input").rglob("languages.json")) else []
ids = [x["id"] for x in ids] if ids and isinstance(ids[0], dict) else ids
import pycountry

def to3(code):
    if len(code) == 3:
        return code
    try:
        return pycountry.languages.get(alpha_2=code.split("-")[0]).alpha_3
    except Exception:
        return None

want = {i: to3(i) for i in ids}
counts = collections.Counter()
short = collections.Counter()
for line in open("/tmp/tatoeba/sentences.csv", encoding="utf-8"):
    parts = line.rstrip("\n").split("\t")
    if len(parts) < 3:
        continue
    counts[parts[1]] += 1
    if 2 <= len(parts[2].split()) <= 6:
        short[parts[1]] += 1
out["tatoeba_by_our_language"] = {i: {"iso3": c, "sentences": counts.get(c, 0), "short": short.get(c, 0)} for i, c in want.items()}
out["tatoeba_total_languages"] = len(counts)
(W / "probe.json").write_text(json.dumps(out, ensure_ascii=False, indent=1), encoding="utf-8")
print("done", flush=True)
