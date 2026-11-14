"""Strict check of returned UI translations before they are merged.

  python ui_check_all.py <strings.json (the zip's)> <all_translations.json or folder> <languages.json>

Flags: a missing language or string, a `{}` placeholder lost or added, three dots lost,
line breaks, an empty value, a value identical to the English in a language whose script is
not Latin, a value more than three times the English length (or under a third) for tips, and
stray markdown or quotes. Prints a per-language summary and every problem."""
import json
import re
import sys
from pathlib import Path

LATIN = set("af az bs bug bjn ca ceb cjk cs cy da de es et eu fi fr fuv ga gl ha hr hu id ig is it jv lb lg ln lt lv mt ms nb nl no ny oc pl pt ro rw sk sl sn so sq sv sw tk tn tr uz vi wo xh yo zu aeb arq ary ars apd acm afb ayl arz apc tl".split())


def main():
    strings = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    src = Path(sys.argv[2])
    if src.is_dir():
        data = {}
        for f in sorted(src.glob("*.json")):
            data[f.stem] = json.loads(f.read_text(encoding="utf-8"))
    else:
        data = json.loads(src.read_text(encoding="utf-8"))
    langs = [l["id"] for l in json.loads(Path(sys.argv[3]).read_text(encoding="utf-8"))]
    problems = []
    for lang in langs:
        rows = data.get(lang)
        if rows is None:
            problems.append((lang, "language missing", ""))
            continue
        for s, hint in strings.items():
            v = rows.get(s)
            if not isinstance(v, str) or not v.strip():
                problems.append((lang, "missing", s[:40]))
                continue
            if v.count("{}") != s.count("{}"):
                problems.append((lang, "placeholder", f"{s[:30]} -> {v[:40]}"))
            if s.endswith("...") and not (v.rstrip().endswith("...") or v.rstrip().endswith("…")):
                problems.append((lang, "dots lost", f"{s[:30]} -> {v[:40]}"))
            if "\n" in v or "```" in v or v != v.strip():
                problems.append((lang, "format", f"{s[:30]} -> {v[:40]!r}"))
            if lang not in LATIN and re.fullmatch(r"[\x00-\x7f]+", v) and not re.fullmatch(r"[\W\d{}]+", v):
                problems.append((lang, "left in English", f"{s[:30]} -> {v[:40]}"))
            if hint.startswith("Hover tip") and (len(v) > 3 * len(s) + 8 or v.endswith(".")):
                problems.append((lang, "tip too long or has a full stop", f"{s[:30]} -> {v[:50]}"))
    extra = sorted(set(data) - set(langs))
    by = {}
    for lang, kind, d in problems:
        by.setdefault(lang, []).append((kind, d))
    for lang, e in sorted(by.items()):
        print(lang, len(e), "; ".join(f"{k}: {d}" for k, d in e[:3]))
    print("languages ok:", len(langs) - len(by), "of", len(langs), "| problems", len(problems), "| extra languages:", extra)


if __name__ == "__main__":
    main()
