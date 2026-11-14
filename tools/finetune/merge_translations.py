"""Merges per-language translation files into one dict and checks them.

  python merge_translations.py <dir with <lang>.json> <scenarios.json> <out.json>

A file may hold {sid: {...}} or {lang: {sid: {...}}}. Prints every problem."""
import json
import re
import sys
from pathlib import Path

folder, scen_path, out_path = map(Path, sys.argv[1:4])
scen = json.loads(scen_path.read_text(encoding="utf-8"))
merged = {}
for f in sorted(folder.glob("*.json")):
    data = json.loads(f.read_text(encoding="utf-8"))
    lang = f.stem
    if lang in data and isinstance(data[lang], dict):
        data = data[lang]
    merged[lang] = data

PH = re.compile(r"\{d[+-]\d+(?:@[0-9:]+)?\}")
bad = {}
for lang, rows in merged.items():
    for sid, s in scen.items():
        r = rows.get(sid)
        why = None
        if not r:
            why = "missing"
        else:
            req, titles = r.get("request", ""), r.get("titles", {})
            if not req.strip():
                why = "empty request"
            elif any(k not in titles or not str(titles[k]).strip() for k in s["titles"]):
                why = "title missing"
            elif any(titles[k] not in req for k in s["titles_used_in_request"]):
                why = "title not in request"
            elif any(p not in req for p in PH.findall(s["request"])):
                why = "placeholder lost"
            elif any('"' in str(v) or "\n" in str(v) for v in titles.values()):
                why = "quote or newline in title"
            elif req.strip() == s["request"].strip() and lang != "en":
                why = "request left in English"
        if why:
            bad.setdefault(lang, []).append((sid, why))
out_path.write_text(json.dumps(merged, ensure_ascii=False), encoding="utf-8")
print(len(merged), "languages merged;", sum(len(v) for v in bad.values()), "problems in", len(bad), "languages")
for lang, items in list(bad.items())[:20]:
    print(" ", lang, items[:4])
