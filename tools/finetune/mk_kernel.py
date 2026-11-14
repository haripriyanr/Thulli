"""Builds a Kaggle script kernel folder: env header + kaggle_train.py + metadata.
  python mk_kernel.py <folder> <slug> KEY=VALUE ..."""
import json, os, sys
from pathlib import Path
folder, slug = Path(sys.argv[1]), sys.argv[2]
env = dict(a.split("=", 1) for a in sys.argv[3:])
folder.mkdir(parents=True, exist_ok=True)
head = "import os\n" + "".join(f"os.environ[{k!r}] = {v!r}\n" for k, v in env.items())
(folder / "run.py").write_text(head + (Path(__file__).parent / "kaggle_train.py").read_text(encoding="utf-8"), encoding="utf-8")
json.dump({"id": f"notharipriyan/{slug}", "title": slug, "code_file": "run.py", "language": "python", "kernel_type": "script",
           "is_private": True, "enable_gpu": True, "enable_internet": True,
           "dataset_sources": ["notharipriyan/thulli-finetune-data"], "competition_sources": [], "kernel_sources": [k for k in os.environ.get("KERNEL_SOURCES", "").split(",") if k], "model_sources": []},
          open(folder / "kernel-metadata.json", "w"), indent=1)
print("ok", folder)
