"""Merges returned UI translations into the .po files.

  python merge_ui.py returned.json          ({lang: {english: translation}})

Fills an empty msgstr, or appends a new entry when the msgid is missing.
Entries that already have a translation are left alone."""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent.parent


def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def main():
    data = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    added = skipped = 0
    for lang, rows in data.items():
        po = ROOT / "lang" / lang / "LC_MESSAGES" / "thulli.po"
        if not po.exists():
            print("no file for", lang)
            continue
        text = po.read_text(encoding="utf-8")
        for en, tr in rows.items():
            tr = str(tr).strip()
            if not tr or en.count("{}") != tr.count("{}"):
                skipped += 1
                continue
            empty = re.compile(r'(msgid "' + re.escape(esc(en)) + r'"\s*\nmsgstr )""')
            if empty.search(text):
                text = empty.sub(lambda m: m.group(1) + '"' + esc(tr) + '"', text, count=1)
            elif ('msgid "' + esc(en) + '"') not in text:
                text = text.rstrip("\n") + f'\n\nmsgid "{esc(en)}"\nmsgstr "{esc(tr)}"\n'
            else:
                continue
            added += 1
        po.write_text(text, encoding="utf-8")
    print("added", added, "skipped", skipped)


if __name__ == "__main__":
    main()
