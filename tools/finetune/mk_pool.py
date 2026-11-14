"""Builds the CPU pool kernel folder (work/k_pool). Usage: python mk_pool.py"""
import json
from pathlib import Path

import package_for_gemini

here = Path(__file__).parent
ids = sorted({l["id"] for l in package_for_gemini.languages()} | {"en"})
src = (here / "pool_data.py").read_text(encoding="utf-8").replace("__IDS__", repr(ids))
folder = here / "work" / "k_pool"
folder.mkdir(parents=True, exist_ok=True)
(folder / "run.py").write_text(src, encoding="utf-8")
json.dump(
    {"id": "notharipriyan/thulli-ft-pool", "title": "thulli-ft-pool", "code_file": "run.py", "language": "python", "kernel_type": "script",
     "is_private": True, "enable_gpu": False, "enable_internet": True, "dataset_sources": [], "competition_sources": [], "kernel_sources": [], "model_sources": []},
    open(folder / "kernel-metadata.json", "w"), indent=1,
)
print("ok", len(ids), "languages")
