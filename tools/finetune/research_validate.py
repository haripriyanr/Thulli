"""Checks the English research scenarios and keeps only the sound ones.

  python research_validate.py <dir with a*.json> <out.json>

A scenario is kept when: it has 3 chunks (2 useful, 1 useless with null
facts); every number in a chunk's facts also appears in its text; the answer
cites only [1] and [2], every sentence has a tag, and it adds no number the
facts lack; the merged notes keep both tags; queries are short and few."""
import json
import re
import sys
from pathlib import Path

NUM = re.compile(r"\d[\d,\.]*\d|\d")
TAG = re.compile(r"\[(\d+)\]")


def numbers(text):
    return {n.rstrip(".,") for n in NUM.findall(text)}


def sentences(text):
    return [s.strip() for s in re.split(r"(?<=[.!?])\s+(?=[A-Z\[])", text.strip()) if s.strip()]


def normalize_tags(answer):
    """Tags go before the full stop: 'text. [1]' becomes 'text [1].', as the app's prompt shows."""
    return re.sub(r"\s*([.!?])\s*((?:\[\d+\])+)", r" \2\1", answer).strip()


def check(s):
    errors = []
    if isinstance(s.get("answer"), str):
        s["answer"] = normalize_tags(s["answer"])
    try:
        q, queries, chunks, gap, merged, answer = s["question"], s["queries"], s["chunks"], s["gap_missing"], s["merged"], s["answer"]
    except KeyError as e:
        return [f"missing key {e}"]
    if not (6 <= len(q.split()) <= 26):
        errors.append("question length")
    if not (2 <= len(queries) <= 3) or any(not (1 <= len(x.split()) <= 8) for x in queries):
        errors.append("queries")
    if not (1 <= len(gap) <= 2) or any(not (1 <= len(x.split()) <= 8) for x in gap):
        errors.append("gap_missing")
    if len(chunks) != 3:
        return errors + ["chunk count"]
    for i, c in enumerate(chunks[:2]):
        if not c.get("facts") or not (40 <= len(c["text"].split()) <= 170):
            errors.append(f"chunk {i + 1} shape")
            continue
        if not numbers(c["facts"]) <= numbers(c["text"]):
            errors.append(f"chunk {i + 1} facts have numbers the text lacks")
    if chunks[2].get("facts") is not None or len(chunks[2]["text"].split()) < 30:
        errors.append("chunk 3 must be useless with null facts")
    tags = {int(t) for t in TAG.findall(answer)}
    if not tags or not tags <= {1, 2}:
        errors.append("answer tags")
    sents = sentences(answer)
    if not (3 <= len(sents) <= 8) or any(not TAG.search(x) for x in sents):
        errors.append("every answer sentence needs a tag")
    allowed = numbers(chunks[0].get("facts") or "") | numbers(chunks[1].get("facts") or "") | {"1", "2"}
    if not numbers(TAG.sub("", answer)) <= allowed:
        errors.append("answer has numbers the facts lack")
    if "[1]" not in merged or "[2]" not in merged or len(merged) >= len(f"[1] {chunks[0].get('facts')} [2] {chunks[1].get('facts')}"):
        errors.append("merged notes")
    return errors


def main():
    folder, out = Path(sys.argv[1]), Path(sys.argv[2])
    good, bad = [], 0
    for f in sorted(folder.glob("a*.json")):
        try:
            items = json.loads(f.read_text(encoding="utf-8"))
        except Exception as e:
            print(f.name, "not valid json:", str(e)[:80])
            continue
        for s in items:
            errs = check(s)
            if errs:
                bad += 1
                print(f.name, s.get("id"), errs)
            else:
                good.append(s)
    out.write_text(json.dumps(good, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"kept {len(good)}, dropped {bad}")


if __name__ == "__main__":
    main()
