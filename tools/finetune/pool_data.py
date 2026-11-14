"""Kaggle job (CPU): builds real-text pools from MASSIVE and Tatoeba.

Outputs (small files, /kaggle/working):
  pools.json         {lang: {"events": [...], "sentences": [...]}}   real task titles
  massive_rows.jsonl {lang, partition, utt, title}                   real requests whose
                     only labelled part is an event name (no dates, people or places)

Datasets: MASSIVE 1.1 (Amazon, CC BY 4.0) and Tatoeba sentences (CC BY 2.0 FR).
Everything is downloaded and processed on Kaggle."""
import collections
import json
import random
import re
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

IDS = __IDS__
W = Path("/kaggle/working")
W.mkdir(exist_ok=True, parents=True)
random.seed(5)
subprocess.run([sys.executable, "-m", "pip", "install", "-q", "pycountry"], check=False)
import pycountry  # noqa: E402

# Tatoeba uses these codes where our id differs from the ISO 639-3 default.
OVERRIDE = {
    "fa": ["pes"], "lv": ["lvs"], "ms": ["zsm", "zlm"], "ne": ["npi"], "sw": ["swh"], "zh": ["cmn"], "ar": ["arb"], "et": ["ekk"],
    "mn": ["khk"], "az": ["azj"], "uz": ["uzn"], "sq": ["als"], "no": ["nob"], "nb": ["nob"], "ps": ["pbt"], "tl": ["tgl", "fil"],
    "ku": ["kmr"], "yue": ["yue"], "hi": ["hin"], "kk": ["kaz"], "ky": ["kir"],
}


def codes(i):
    out = list(OVERRIDE.get(i, []))
    if len(i) == 3:
        out.append(i)
    else:
        try:
            out.append(pycountry.languages.get(alpha_2=i).alpha_3)
        except Exception:
            pass
    return out


def fetch(url, dest):
    print("downloading", url, flush=True)
    urllib.request.urlretrieve(url, dest)
    print("  got", Path(dest).stat().st_size, "bytes", flush=True)


pools = {i: {"events": [], "sentences": []} for i in IDS}

# ---- MASSIVE
fetch("https://amazon-massive-nlu-dataset.s3.amazonaws.com/amazon-massive-dataset-1.1.tar.gz", "/tmp/massive.tar.gz")
with tarfile.open("/tmp/massive.tar.gz") as t:
    t.extractall("/tmp/massive")
SLOT = re.compile(r"\[(\w+) : ([^\]]+)\]")
rows_out = []
for f in sorted(Path("/tmp/massive").rglob("*.jsonl")):
    loc = f.stem
    if loc == "zh-TW":
        continue
    lang = loc.split("-")[0]
    lang = {"nb": "nb", "zh": "zh"}.get(lang, lang)
    if lang not in pools:
        continue
    events = set()
    for line in open(f, encoding="utf-8"):
        r = json.loads(line)
        if r["scenario"] != "calendar":
            continue
        slots = SLOT.findall(r["annot_utt"])
        for k, v in slots:
            if k == "event_name" and 2 <= len(v) <= 40:
                events.add(v.strip())
        if r["intent"] == "calendar_set" and len(slots) == 1 and slots[0][0] == "event_name":
            title = slots[0][1].strip()
            if title and title.lower() in r["utt"].lower() and 2 <= len(title) <= 40:
                rows_out.append({"lang": lang, "partition": r["partition"], "utt": r["utt"], "title": title})
    pools[lang]["events"] = sorted(events)

# ---- Tatoeba
fetch("https://downloads.tatoeba.org/exports/sentences.tar.bz2", "/tmp/sentences.tar.bz2")
with tarfile.open("/tmp/sentences.tar.bz2") as t:
    t.extractall("/tmp/tatoeba")
wanted = {}
for i in IDS:
    for c in codes(i):
        wanted.setdefault(c, []).append(i)
by_code = collections.defaultdict(list)
UNSPACED = set("zh yue ja th km my bo lo".split())
for line in open("/tmp/tatoeba/sentences.csv", encoding="utf-8"):
    parts = line.rstrip("\n").split("\t")
    if len(parts) < 3 or parts[1] not in wanted:
        continue
    s = parts[2].strip().rstrip(".")
    if any(ch.isdigit() for ch in s) or any(ch in s for ch in '?!"“”«»:;()[]'):
        continue
    by_code[parts[1]].append(s)
for code, sents in by_code.items():
    for i in wanted[code]:
        if pools[i]["sentences"]:
            continue
        spaced = i not in UNSPACED
        ok = [s for s in sents if (2 <= len(s.split()) <= 7 and len(s) <= 48) or (not spaced and 3 <= len(s) <= 24)]
        random.shuffle(ok)
        pools[i]["sentences"] = ok[:400]

(W / "pools.json").write_text(json.dumps(pools, ensure_ascii=False), encoding="utf-8")
with open(W / "massive_rows.jsonl", "w", encoding="utf-8") as f:
    for r in rows_out:
        f.write(json.dumps(r, ensure_ascii=False) + "\n")
summary = {
    "languages_with_events": sum(1 for p in pools.values() if p["events"]),
    "languages_with_sentences": sum(1 for p in pools.values() if p["sentences"]),
    "languages_with_neither": sorted(i for i, p in pools.items() if not p["events"] and not p["sentences"]),
    "massive_rows": len(rows_out),
    "massive_rows_by_partition": dict(collections.Counter(r["partition"] for r in rows_out)),
    "sentences_per_language": {i: len(p["sentences"]) for i, p in pools.items()},
    "events_per_language": {i: len(p["events"]) for i, p in pools.items() if p["events"]},
}
(W / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=1), encoding="utf-8")
print(json.dumps({k: v for k, v in summary.items() if "per_language" not in k}, ensure_ascii=False), flush=True)
