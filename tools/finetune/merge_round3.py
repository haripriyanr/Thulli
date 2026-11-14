"""Joins the rendered round 3 rows into one rows.jsonl for the Kaggle dataset.

  python merge_round3.py <rows_main.jsonl> <rows_research.jsonl> <out rows.jsonl> [templated_eval_per_cell]

Keeps every training row, every held-out native, MASSIVE and research row, and at most
N (default 4) held-out translated-frame rows per language and domain."""
import collections
import json
import sys


def main():
    main_rows = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8") if l.strip()]
    research = [json.loads(l) for l in open(sys.argv[2], encoding="utf-8") if l.strip()]
    per_cell = int(sys.argv[4]) if len(sys.argv) > 4 else 4
    out, cell = [], collections.Counter()
    for r in main_rows:
        if not r.get("gold_ok", True):
            continue
        if r["split"] == "eval" and r["suite"] == "templated":
            k = (r["lang"], r["domain"])
            if cell[k] >= per_cell:
                continue
            cell[k] += 1
        out.append(r)
    for r in research:
        r["suite"] = "research"
        out.append(r)
    with open(sys.argv[3], "w", encoding="utf-8") as f:
        for r in out:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    c = collections.Counter((r["suite"], r["split"]) for r in out)
    print(len(out), "rows", dict(sorted(c.items())))


if __name__ == "__main__":
    main()
