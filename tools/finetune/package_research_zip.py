"""Builds thulli-research-translation.zip: the language-dependent parts of the
research scenarios, to translate. Page text and notes stay English (real pages
usually are), so only the question, searches and the answer are needed.

  python package_research_zip.py <valid.json> <out_dir>
"""
import json
import sys
import zipfile
from pathlib import Path

PROMPT = """# Translation job: research examples for the Thulli assistant

Thulli's on-device AI does web research for a person. We teach it with examples in every
supported language. For each scenario you get an English question, the web searches that
answer it, searches for a missing part, and a short answer that cites its sources with tags
like [1] and [2]. Translate these into every language in `languages.json` (`en` is not needed).

## Output
One JSON object per language, shaped like `example_output.json`:
{"<language id>": {"<scenario id>": {"question": "...", "queries": ["..."], "gap_missing": ["..."], "answer": "..."}, ...}}

## Rules (these matter more than elegance)
1. `question`: what a real person would type, natural and short, in the standard script of the language.
2. `queries` and `gap_missing`: short web search queries (2 to 7 words) as a person would type into a search box in that language. Keep proper names and technical terms people really search with. Same number of items as the English.
3. `answer`: translate it faithfully and fluently. KEEP the source tags exactly as written, like [1] and [2], at the end of the same sentences, with ASCII digits and square brackets. Do not add, remove or move tags.
4. KEEP every number exactly as in the English, written with plain ASCII digits 0-9 (also in languages that use other numerals).
5. Do not add facts, do not leave out facts, do not add notes about the translation.
6. No double quotes inside the strings except the JSON ones; no line breaks inside a string.

## How to work
Do 4 or 5 languages per reply. Put each language in its own code block. If a reply is cut off, continue with the scenarios that are missing for that language.
"""

CHECK = r'''"""Usage: python check_research.py research_scenarios.json translations.json"""
import json, re, sys

src = json.load(open(sys.argv[1], encoding="utf-8"))
data = json.load(open(sys.argv[2], encoding="utf-8"))
TAG = re.compile(r"\[(\d+)\]")
NUM = re.compile(r"\d[\d,\.]*\d|\d")
bad = 0
for lang, rows in data.items():
    for sid, s in src.items():
        r = rows.get(sid)
        if not r:
            print(lang, sid, "missing"); bad += 1; continue
        for k in ("queries", "gap_missing"):
            if len(r.get(k, [])) != len(s[k]) or any(not str(x).strip() for x in r.get(k, [])):
                print(lang, sid, k, "wrong count"); bad += 1
        if not str(r.get("question", "")).strip():
            print(lang, sid, "question empty"); bad += 1
        a = r.get("answer", "")
        if sorted(TAG.findall(a)) != sorted(TAG.findall(s["answer"])):
            print(lang, sid, "tags differ"); bad += 1
        if {n.rstrip(".,") for n in NUM.findall(TAG.sub("", s["answer"]))} - {n.rstrip(".,") for n in NUM.findall(TAG.sub("", a))}:
            print(lang, sid, "a number is missing"); bad += 1
print("problems:", bad)
'''


def languages():
    import package_for_gemini

    return [l for l in package_for_gemini.languages() if l["id"] != "en"]


def main():
    valid = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    src = {s["id"]: {"question": s["question"], "queries": s["queries"], "gap_missing": s["gap_missing"], "answer": s["answer"]} for s in valid}
    example = {"es": {k: {"question": "...", "queries": ["..."] * len(v["queries"]), "gap_missing": ["..."] * len(v["gap_missing"]), "answer": "... [1] ... [2]"} for k, v in list(src.items())[:1]}}
    z = out / "thulli-research-translation.zip"
    with zipfile.ZipFile(z, "w", zipfile.ZIP_DEFLATED) as f:
        f.writestr("PROMPT.md", PROMPT)
        f.writestr("research_scenarios.json", json.dumps(src, ensure_ascii=False, indent=1))
        f.writestr("languages.json", json.dumps(languages(), ensure_ascii=False))
        f.writestr("check_research.py", CHECK)
        f.writestr("example_output.json", json.dumps(example, ensure_ascii=False, indent=1))
    (out / "research_src.json").write_text(json.dumps(src, ensure_ascii=False), encoding="utf-8")
    print(z, len(src), "scenarios,", len(languages()), "languages")


if __name__ == "__main__":
    main()
