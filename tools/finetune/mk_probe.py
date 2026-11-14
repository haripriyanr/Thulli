"""Builds the CPU probe kernel folder (work/k_probe). Usage: python mk_probe.py"""
import json
import re
from pathlib import Path

import package_for_gemini

here = Path(__file__).parent
ids = [l["id"] for l in package_for_gemini.languages()] + ["en"]
src = (here / "probe_data.py").read_text(encoding="utf-8")
src = re.sub(r"ids = json\.loads\(Path.*?\n", f"ids = {ids!r}\n", src, count=1)
src = re.sub(r"ids = \[x\[\"id\"\].*?\n", "", src, count=1)
folder = here / "work" / "k_probe"
folder.mkdir(parents=True, exist_ok=True)
(folder / "run.py").write_text(src, encoding="utf-8")
json.dump(
    {"id": "notharipriyan/thulli-ft-probe", "title": "thulli-ft-probe", "code_file": "run.py", "language": "python", "kernel_type": "script",
     "is_private": True, "enable_gpu": False, "enable_internet": True, "dataset_sources": [], "competition_sources": [], "kernel_sources": [], "model_sources": []},
    open(folder / "kernel-metadata.json", "w"), indent=1,
)
print("ok", len(ids), "languages")
