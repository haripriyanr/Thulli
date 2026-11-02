#!/usr/bin/env python3
"""Packs the files the Kaggle notebook needs into one zip (a Kaggle dataset).

The notebook compiles the eval harness against the app's real prompt, action and
model sources, so those travel with the tools. Upload the zip as a private dataset
named `thulli-assistant-src`, or skip this and let the notebook `git clone` a repo.

    python tools/assistant_ft/make_kaggle_bundle.py   # -> tools/assistant_ft/thulli-assistant-src.zip
"""
import os
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
INCLUDE = ["src/models", "src/services/assistant", "tools/assistant_ft"]
SKIP_DIRS = {"target", "data", "__pycache__", ".ipynb_checkpoints"}
SKIP_FILES = {"thulli-assistant-src.zip"}
# Only these files of src/services/assistant are compiled by the harness.
ASSISTANT_KEEP = {"actions.rs", "prompt.rs", "engine.rs", "local.rs", "remote.rs"}


def main():
    out = os.path.join(HERE, "thulli-assistant-src.zip")
    n = 0
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for inc in INCLUDE:
            for dirpath, dirs, files in os.walk(os.path.join(ROOT, inc)):
                dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
                for f in files:
                    if f in SKIP_FILES or f.endswith((".pyc", ".zip")):
                        continue
                    if inc == "src/services/assistant" and f not in ASSISTANT_KEEP:
                        continue
                    path = os.path.join(dirpath, f)
                    z.write(path, os.path.relpath(path, ROOT).replace(os.sep, "/"))
                    n += 1
    print(f"wrote {out} ({n} files)")


if __name__ == "__main__":
    main()
