"""Lists UI strings (@tr) that some language file lacks a translation for.
  python ui_missing.py            -> prints counts
  python ui_missing.py out.json   -> writes {lang: [msgid, ...]}"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent.parent
STR = r'"((?:[^"\\]|\\.)*)"'


def unesc(s):
    return s.replace('\\"', '"').replace("\\n", "\n").replace("\\\\", "\\")


src = set()
for p in (ROOT / "ui").rglob("*.slint"):
    for m in re.finditer(r"@tr\(" + STR, p.read_text(encoding="utf-8")):
        src.add(unesc(m.group(1)))


def po_entries(path):
    text = path.read_text(encoding="utf-8")
    out = {}
    block = r"((?:" + STR + r"\s*)+)"
    for m in re.finditer(r"msgid\s+" + block + r"msgstr\s+" + block, text):
        j = lambda s: unesc("".join(re.findall(STR, s)))
        out[j(m.group(1))] = j(m.group(3))
    return out


missing = {}
for po in sorted((ROOT / "lang").glob("*/LC_MESSAGES/thulli.po")):
    lang = po.parent.parent.name
    have = po_entries(po)
    miss = sorted(s for s in src if not have.get(s))
    if miss and lang != "en":
        missing[lang] = miss
print(len(src), "strings in the UI;", len(missing), "languages missing some")
allmiss = sorted({s for v in missing.values() for s in v})
print(len(allmiss), "distinct strings missing somewhere")
for s in allmiss[:40]:
    print("  ", s[:80])
if len(sys.argv) > 1:
    json.dump(missing, open(sys.argv[1], "w", encoding="utf-8"), ensure_ascii=False, indent=0)
