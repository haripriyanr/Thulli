"""Builds thulli-finetune-translation.zip: the scenarios to translate, the
language list, the prompt and a checker the returned files are validated with.

  python package_for_gemini.py <out_dir>
"""
import json
import re
import sys
import zipfile
from pathlib import Path

import skeletons

HERE = Path(__file__).parent
ROOT = HERE.parent.parent


def languages():
    s = (ROOT / "src/services/preferences.rs").read_text(encoding="utf-8")
    block = s[s.index("pub const LANGUAGE_CHOICES"):]
    block = block[: block.index("];")]
    return [{"id": i, "name": n} for i, n, _ in re.findall(r'\("([^"]+)",\s*"([^"]+)",\s*"([^"]+)"\)', block)]


def compact(sks):
    """Only what a translator needs."""
    out = {}
    for s in sks:
        in_req = [k for k, v in s["titles"].items() if v in s["request_en"]]
        out[s["sid"]] = {"request": s["request_en"], "titles": s["titles"], "titles_used_in_request": in_req}
    return out


PROMPT = """# Translation job for the Thulli planner (fine-tuning data)

Thulli is a productivity app. A small on-device AI turns what a person types into actions. We are teaching it
with realistic requests in every language the app supports. Your job is to translate request sentences and
the short titles inside them, so the AI learns to read each language.

## What is in this zip
- `scenarios.json`: {scenario_id: {"request": English sentence, "titles": {KEY: English text}, "titles_used_in_request": [KEYS]}}
- `languages.json`: the languages to translate into (id and English name). `en` is not needed.
- `check_translation.py`: run on your output to find mistakes.
- `example_output.json`: the exact output shape, shown for Spanish.

## What to produce, per language
One JSON object per language, shaped like `example_output.json`:
{"<language id>": {"<scenario id>": {"request": "<natural sentence>", "titles": {"T1": "<translated text>", ...}}, ...}}

## Rules (these matter more than elegance)
1. Write what a real person would type to their planner, in everyday language: short, casual, natural for that language and script.
2. Translate every entry in `titles` into the language (short noun/verb phrases, like a to-do item).
3. Every key listed in `titles_used_in_request` must appear in your `request` EXACTLY, character for character, as you wrote it in `titles`.
   (Example: titles {"T1": "Escribir el informe"} means the request contains "Escribir el informe" unchanged, even if grammar would prefer another form: rephrase the sentence around it.)
   Keys that are not listed there (such as M1, K11, N1, B1) are translated but do not need to appear in the request.
4. Keep placeholders such as {d+3}, {d-1}, {d+7} exactly as written, in the request. Never translate or reformat them.
5. Keep digits as plain ASCII digits (0-9), also in languages that use other numerals. Keep times like 14:00 and 09:30 as written.
6. Keep the meaning identical: same number of minutes, same priority word (urgent/high/medium/low), same day words (today, tomorrow, next week, yesterday), same steps in the same order.
7. Do not add or remove tasks, steps or sentences. Do not answer the request; only translate it.
8. Titles must not contain double quotes or line breaks.
9. Use the standard written script of the language (for example Cyrillic for Kazakh, Arabic script for Urdu, Devanagari for Hindi). Do not transliterate to Latin unless that is how the language is normally typed in apps.
10. For `B1` (a note body) translate the whole sentence and keep every number.

## How to work
Translate 4 or 5 languages per reply (about 190 scenarios each) so replies are not cut off.
If a reply is cut off, continue with the scenarios that are missing for that language; do not restart.
Put each language in its own code block so it can be copied cleanly.
When you are unsure about a low-resource language, still translate; do not skip scenarios.
"""

CHECK = '''"""Usage: python check_translation.py scenarios.json translations.json
Reports every scenario where a rule is broken."""
import json, re, sys

scen = json.load(open(sys.argv[1], encoding="utf-8"))
data = json.load(open(sys.argv[2], encoding="utf-8"))
bad = 0
for lang, rows in data.items():
    for sid, s in scen.items():
        r = rows.get(sid)
        if not r:
            print(lang, sid, "missing"); bad += 1; continue
        req, titles = r.get("request", ""), r.get("titles", {})
        for k in s["titles"]:
            if k not in titles or not str(titles[k]).strip():
                print(lang, sid, "title missing", k); bad += 1
        for k in s["titles_used_in_request"]:
            if k in titles and titles[k] not in req:
                print(lang, sid, "title not in request", k); bad += 1
        for ph in re.findall(r"\\{d[+-]\\d+(?:@[0-9:]+)?\\}", s["request"]):
            if ph not in req:
                print(lang, sid, "placeholder lost", ph); bad += 1
        if any('"' in str(v) or "\\n" in str(v) for v in titles.values()):
            print(lang, sid, "quote or newline in a title"); bad += 1
print("problems:", bad)
'''


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    sks = skeletons.build()
    langs = [l for l in languages() if l["id"] != "en"]
    scen = compact(sks)
    example = {"es": {sid: {"request": "...", "titles": {k: "..." for k in v["titles"]}} for sid, v in list(scen.items())[:2]}}
    zpath = out / "thulli-finetune-translation.zip"
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("PROMPT.md", PROMPT)
        z.writestr("scenarios.json", json.dumps(scen, ensure_ascii=False, indent=1))
        z.writestr("languages.json", json.dumps(langs, ensure_ascii=False, indent=0))
        z.writestr("check_translation.py", CHECK)
        z.writestr("example_output.json", json.dumps(example, ensure_ascii=False, indent=1))
    json.dump(sks, open(out / "skeletons.json", "w", encoding="utf-8"), ensure_ascii=False)
    print(zpath, len(scen), "scenarios,", len(langs), "languages")


if __name__ == "__main__":
    main()
