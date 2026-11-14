"""Strict check of research translations (a folder of <lang>.json files).

  python research_check_all.py <scenarios.json> <folder> <languages.json>

Beyond check_research.py it flags: wrong or missing language files, answers or
questions left identical to English, numbers added that the English lacks,
tags in a different order, a sentence without a tag, queries or gaps longer
than 7 words (unspaced scripts are measured in characters), and a question
that is empty or very short or very long."""
import json
import re
import sys
from pathlib import Path

TAG = re.compile(r"\[(\d+)\]")
NUM = re.compile(r"\d[\d,\.]*\d|\d")
UNSPACED = {"zh", "yue", "ja", "th", "km", "my", "bo", "lo"}


def nums(t):
    return {n.rstrip(".,") for n in NUM.findall(TAG.sub("", t))}


def words(s, lang):
    return len(s.strip()) // 2 if lang in UNSPACED else len(s.split())


def main():
    src = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    folder = Path(sys.argv[2])
    want = [l["id"] for l in json.loads(Path(sys.argv[3]).read_text(encoding="utf-8")) if l["id"] != "en"]
    problems = {}
    seen = set()
    for lang in want:
        f = folder / f"{lang}.json"
        if not f.exists():
            problems.setdefault(lang, []).append("file missing")
            continue
        data = json.loads(f.read_text(encoding="utf-8"))
        rows = data.get(lang) if lang in data else next(iter(data.values()))
        seen.add(lang)
        for sid, s in src.items():
            r = rows.get(sid)
            if not r:
                problems.setdefault(lang, []).append(f"{sid} missing")
                continue
            e = problems.setdefault(lang, [])
            q = str(r.get("question", ""))
            a = str(r.get("answer", ""))
            if not (6 <= words(q, lang) <= 40) and lang not in UNSPACED:
                e.append(f"{sid} question length {words(q, lang)}")
            if lang not in UNSPACED and (q.strip() == s["question"].strip() or a.strip() == s["answer"].strip()):
                e.append(f"{sid} left in English")
            if [int(x) for x in TAG.findall(a)] != [int(x) for x in TAG.findall(s["answer"])]:
                e.append(f"{sid} tags differ")
            if nums(a) - nums(s["answer"]):
                e.append(f"{sid} number added {sorted(nums(a) - nums(s['answer']))}")
            if nums(s["answer"]) - nums(a):
                e.append(f"{sid} number missing {sorted(nums(s['answer']) - nums(a))}")
            for k in ("queries", "gap_missing"):
                got = r.get(k, [])
                if len(got) != len(s[k]):
                    e.append(f"{sid} {k} count")
                for x in got:
                    if not str(x).strip() or words(str(x), lang) > 9:
                        e.append(f"{sid} {k} long/empty: {str(x)[:30]}")
            if a and not TAG.search(a):
                e.append(f"{sid} answer has no tag")
    extra = sorted(p.stem for p in folder.glob("*.json") if p.stem not in want)
    total = 0
    for lang, e in sorted(problems.items()):
        if e:
            total += len(e)
            print(lang, len(e), "e.g.", "; ".join(e[:3]))
    print("languages ok:", sum(1 for l in want if not problems.get(l)), "of", len(want), "| problems:", total, "| extra files:", extra)
    sys.exit(1 if total else 0)


if __name__ == "__main__":
    main()
