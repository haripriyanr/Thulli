"""Merges research translations (a folder of <lang>.json) into one file of sound scenarios.

  python research_merge.py <research_valid.json> <folder> <languages.json> <out.json>

A scenario is kept for a language when: it exists, the tags are the same and in the
same order as the English, every English number is present, every number the
translation adds also appears in the scenario's chunks (so a model could have read it
there; a number like 1890 that appears nowhere is an invention), the answer was
translated, and queries and gaps have the English counts and are not empty.
Prints per-language kept counts; writes {lang: {scenario id: row}}."""
import json
import re
import sys
from pathlib import Path

TAG = re.compile(r"\[(\d+)\]")
NUM = re.compile(r"\d[\d,\.]*\d|\d")
UNSPACED = {"zh", "yue", "ja", "th", "km", "my", "bo", "lo"}
# English answers that spell a small number out, so a translation may write the digit.
SPELLED = re.compile(r"\b(one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|first|second|third|twice|half|dozen)\b", re.I)


def nums(t):
    return {n.rstrip(".,") for n in NUM.findall(TAG.sub("", t))}


def main():
    valid = {s["id"]: s for s in json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))}
    folder = Path(sys.argv[2])
    langs = [l["id"] for l in json.loads(Path(sys.argv[3]).read_text(encoding="utf-8")) if l["id"] != "en"]
    grounded = {}
    for sid, s in valid.items():
        text = json.dumps(s["chunks"], ensure_ascii=False) + s["question"] + s["answer"]
        grounded[sid] = nums(text) | {n.replace(",", "") for n in nums(text)}
    out, report, reasons = {}, {}, {}
    for lang in langs:
        f = folder / f"{lang}.json"
        if not f.exists():
            report[lang] = 0
            continue
        data = json.loads(f.read_text(encoding="utf-8"))
        rows = data.get(lang) or next(iter(data.values()))
        keep = {}
        for sid, s in valid.items():
            r = rows.get(sid)
            why = None
            if not r:
                why = "missing"
            else:
                a, q = str(r.get("answer", "")), str(r.get("question", ""))
                if [int(x) for x in TAG.findall(a)] != [int(x) for x in TAG.findall(s["answer"])]:
                    why = "tags"
                elif nums(s["answer"]) - nums(a):
                    why = "number missing"
                elif any(n not in grounded[sid] and n.replace(",", "") not in grounded[sid] and not (len(n) <= 2 and SPELLED.search(s["answer"])) for n in nums(a)):
                    why = "number invented"
                elif lang not in UNSPACED and (a.strip() == s["answer"].strip() or q.strip() == s["question"].strip()):
                    why = "english"
                elif len(r.get("queries", [])) != len(s["queries"]) or len(r.get("gap_missing", [])) != len(s["gap_missing"]):
                    why = "count"
                elif any(not str(x).strip() for x in r["queries"] + r["gap_missing"]) or not q.strip():
                    why = "empty"
            if why:
                reasons[why] = reasons.get(why, 0) + 1
            else:
                keep[sid] = {k: r[k] for k in ("question", "queries", "gap_missing", "answer")}
        out[lang] = keep
        report[lang] = len(keep)
    Path(sys.argv[4]).write_text(json.dumps(out, ensure_ascii=False), encoding="utf-8")
    total = len(valid)
    low = {l: n for l, n in report.items() if n < total}
    print("languages:", len(report), "| scenarios per language:", total, "| kept rows:", sum(report.values()), "of", total * len(report))
    print("dropped by reason:", reasons)
    print("languages with any drop:", dict(sorted(low.items(), key=lambda kv: kv[1])))


if __name__ == "__main__":
    main()
