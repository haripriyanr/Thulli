#!/usr/bin/env python3
"""Writes kaggle_assistant_finetune.ipynb. Edit the cells here, not the .ipynb."""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
CELLS = []


def md(text):
    CELLS.append({"cell_type": "markdown", "metadata": {}, "source": text.strip("\n").splitlines(True)})


def code(text):
    CELLS.append({"cell_type": "code", "metadata": {}, "execution_count": None, "outputs": [],
                  "source": text.strip("\n").splitlines(True)})


md("""
# Thulli assistant: evaluate and fine-tune Gemma 4 E2B

Runs top to bottom on a Kaggle GPU notebook (internet on):

1. get the source (the app's real prompt, action and model files + the tools)
2. install Rust and build the eval harness (uses the app's own `plan_prompt`, `sort_prompt`, `parse_reply`)
3. generate the multilingual dataset and render prompts with the real app code
4. **baseline eval** of the untouched model
5. LoRA / QLoRA SFT so the model answers with the JSON object only
6. **re-eval**, per-language and per-case-type report
7. export the adapter, the merged model and (if llama.cpp supports it) a GGUF for the `local-model` engine

Everything lands in `/kaggle/working/out`.
""")

code("""
import glob, os, shutil, subprocess, sys

MODEL_ID = "google/gemma-4-E2B-it"
EPOCHS = 2.0            # lower for a smoke run
MAX_TRAIN = 0           # 0 = all examples; e.g. 300 for a quick check
EVAL_LIMIT = 0          # 0 = whole test set; e.g. 100 for a quick check
REPO_URL = ""           # used only when no dataset is attached, e.g. https://github.com/you/thulli.git
WORK = "/kaggle/working"
OUT = f"{WORK}/out"
SRC = f"{WORK}/thulli"
os.makedirs(OUT, exist_ok=True)

def sh(cmd, **kw):
    print("$", cmd)
    subprocess.run("set -o pipefail; " + cmd, shell=True, check=True, executable="/bin/bash", **kw)

# Source: an attached dataset built by tools/assistant_ft/make_kaggle_bundle.py, else a git clone.
hits = glob.glob("/kaggle/input/*/tools/assistant_ft") + glob.glob("/kaggle/input/*/*/tools/assistant_ft")
if os.path.exists(SRC):
    shutil.rmtree(SRC)
if hits:
    shutil.copytree(hits[0].rsplit("/tools/assistant_ft", 1)[0], SRC)
elif REPO_URL:
    sh(f"git clone --depth 1 {REPO_URL} {SRC}")
else:
    raise SystemExit("Attach the thulli-assistant-src dataset or set REPO_URL.")
FT = f"{SRC}/tools/assistant_ft"
print(os.listdir(FT))
""")

code("""
sh("pip install -q -U transformers peft accelerate bitsandbytes datasets")
import torch, transformers
print(torch.__version__, transformers.__version__, torch.cuda.get_device_name(0) if torch.cuda.is_available() else "NO GPU")
""")

code("""
# Rust toolchain and the harness. The harness builds against the app's real source files.
sh("curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal")
os.environ["PATH"] = os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"]
sh(f"cd {FT}/eval && cargo build --release 2>&1 | tail -5")
EVAL = f"{FT}/eval/target/release/assistant_eval"
""")

code("""
# Dataset: generate, then render prompts with the app's own code. Any target that
# parse_reply would reject makes `render --strict` fail.
sh(f"cd {FT} && python gen_dataset.py --out {OUT}/data")
for split in ("train", "test"):
    sh(f"{EVAL} render --cases {OUT}/data/{split}_cases.jsonl --out {OUT}/data/{split}_rendered.jsonl --strict")
# Sanity: the gold answers must score 100% with the real parser.
sh(f"{EVAL} run --cases {OUT}/data/test_cases.jsonl --out {OUT}/oracle.jsonl --oracle 2>/dev/null | head -4")
""")

code("""
import time, urllib.request, signal

SERVER = None
def start_server(model, port=8000):
    global SERVER
    stop_server()
    SERVER = subprocess.Popen([sys.executable, f"{FT}/serve_hf.py", "--model", model, "--port", str(port)])
    for _ in range(600):
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}", timeout=2)
            return
        except Exception:
            if SERVER.poll() is not None:
                raise SystemExit("server exited while loading the model")
            time.sleep(2)
    raise SystemExit("server did not come up")

def stop_server():
    global SERVER
    if SERVER and SERVER.poll() is None:
        SERVER.send_signal(signal.SIGTERM)
        SERVER.wait(60)
    SERVER = None
    torch.cuda.empty_cache()

def evaluate(model, name, extra=""):
    start_server(model)
    limit = f"--limit {EVAL_LIMIT}" if EVAL_LIMIT else ""
    try:
        sh(f"{EVAL} run --cases {OUT}/data/test_cases.jsonl --out {OUT}/results_{name}.jsonl "
           f"--endpoint http://127.0.0.1:8000/v1/chat/completions --model {name} "
           f"--max-tokens 1400 --concurrency 16 {limit} {extra} > {OUT}/summary_{name}.md")
    finally:
        stop_server()
    print(open(f"{OUT}/summary_{name}.md").read().split("### By kind")[0])

# Baseline: the model exactly as the app would call it (chat template defaults).
evaluate(MODEL_ID, "before")
""")

code("""
limit = f"--max-examples {MAX_TRAIN}" if MAX_TRAIN else ""
sh(f"python {FT}/train_lora.py --model {MODEL_ID} --data {OUT}/data/train_rendered.jsonl --out {OUT}/model "
   f"--epochs {EPOCHS} {limit} 2>&1 | grep -v Warning | tail -60")
""")

code("""
evaluate(f"{OUT}/model/merged", "after")
""")

code("""
sh(f"{EVAL} report --before {OUT}/results_before.jsonl --after {OUT}/results_after.jsonl > {OUT}/REPORT.md")
print(open(f"{OUT}/REPORT.md").read())
""")

code("""
# GGUF for the on-device engine. This needs llama.cpp to know the architecture; if
# the converter refuses, the merged Hugging Face model in out/model/merged is still the result.
if not os.path.exists(f"{WORK}/llama.cpp"):
    sh(f"git clone --depth 1 https://github.com/ggml-org/llama.cpp {WORK}/llama.cpp")
sh(f"pip install -q -r {WORK}/llama.cpp/requirements/requirements-convert_hf_to_gguf.txt")
try:
    sh(f"python {WORK}/llama.cpp/convert_hf_to_gguf.py {OUT}/model/merged --outtype q8_0 --outfile {OUT}/thulli-assistant-q8_0.gguf")
except subprocess.CalledProcessError:
    print("GGUF conversion not supported for this architecture yet; use the merged model or retry with a newer llama.cpp.")
""")

code("""
for root, _, files in os.walk(OUT):
    if "trainer" in root:
        continue
    for f in files:
        p = os.path.join(root, f)
        print(f"{os.path.getsize(p)/2**20:9.1f} MB  {os.path.relpath(p, OUT)}")
""")

nb = {"cells": CELLS, "metadata": {"kernelspec": {"display_name": "Python 3", "language": "python", "name": "python3"},
                                   "language_info": {"name": "python"}}, "nbformat": 4, "nbformat_minor": 5}
with open(os.path.join(HERE, "kaggle_assistant_finetune.ipynb"), "w", encoding="utf-8", newline="\n") as f:
    json.dump(nb, f, indent=1, ensure_ascii=False)
print("wrote notebook with", len(CELLS), "cells")
