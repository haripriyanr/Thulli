"""Round 4: scenarios added to skeletons.py after the big translation, packed for Gemini.

  python package_gaps.py <work dir>

Reads <work>/skeletons.json (the saved scenarios, which round 2 edited in place, so the
generator is not re-run for them), appends the scenarios the generator now makes that the
saved file lacks, writes <work>/skeletons_r4.json and <work>/thulli-finetune-translation-r4.zip
with only the new scenarios to translate. Merge the answer into translations.json with
merge_translations.py as before."""
import json
import sys
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import package_for_gemini as pg  # noqa: E402
import skeletons  # noqa: E402


def main():
    work = Path(sys.argv[1])
    old = json.loads((work / "skeletons.json").read_text(encoding="utf-8"))
    have = {s["sid"] for s in old}
    new = [s for s in skeletons.build() if s["sid"] not in have]
    (work / "skeletons_r4.json").write_text(json.dumps(old + new, ensure_ascii=False), encoding="utf-8")
    langs = [l for l in pg.languages() if l["id"] != "en"]
    scen = pg.compact(new)
    prompt = pg.PROMPT.replace(
        "Translate 4 or 5 languages per reply (about 190 scenarios each) so replies are not cut off.",
        "There are only %d short scenarios, so 10 to 15 languages per reply is fine." % len(scen),
    )
    example = {"es": {sid: {"request": "...", "titles": {k: "..." for k in v["titles"]}} for sid, v in list(scen.items())[:2]}}
    z = work / "thulli-finetune-translation-r4.zip"
    with zipfile.ZipFile(z, "w", zipfile.ZIP_DEFLATED) as f:
        f.writestr("PROMPT.md", prompt)
        f.writestr("scenarios.json", json.dumps(scen, ensure_ascii=False, indent=1))
        f.writestr("languages.json", json.dumps(langs, ensure_ascii=False, indent=0))
        f.writestr("check_translation.py", pg.CHECK)
        f.writestr("example_output.json", json.dumps(example, ensure_ascii=False, indent=1))
    print(z, len(scen), "new scenarios,", len(langs), "languages; skeletons_r4.json has", len(old) + len(new))


if __name__ == "__main__":
    main()
