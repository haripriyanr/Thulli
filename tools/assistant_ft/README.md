# Assistant model: eval and fine-tune

Evaluates and fine-tunes **google/gemma-4-E2B-it** to answer Thulli's one-shot plan and sort prompts with the JSON action object only, so it can later be bundled for the `local-model` engine.

| File | What it does |
|---|---|
| `eval/` | Rust harness `assistant_eval`. Includes the app's real `models`, `prompt.rs`, `actions.rs` and `engine.rs` by path, so prompts and parsing cannot drift. Builds on Linux with only serde, chrono, uuid and ureq. |
| `gen_dataset.py`, `langs.py` | Synthetic and templated cases in 28 languages. |
| `serve_hf.py` | OpenAI-compatible batched server over a Hugging Face model, so the harness treats base and tuned models alike. |
| `train_lora.py` | LoRA/QLoRA SFT on the reply only, then merge. |
| `build_notebook.py` -> `kaggle_assistant_finetune.ipynb` | The end-to-end Kaggle run. |
| `make_kaggle_bundle.py` | Zips the sources the notebook needs into a dataset (`thulli-assistant-src`). |

## Run it

Locally (dataset and scoring only, no GPU):

```bash
python tools/assistant_ft/gen_dataset.py --out tools/assistant_ft/data
cd tools/assistant_ft/eval && cargo build --release
./target/release/assistant_eval render --cases ../data/train_cases.jsonl --out ../data/train_rendered.jsonl --strict
./target/release/assistant_eval run --cases ../data/test_cases.jsonl --out ../data/oracle.jsonl --oracle   # gold answers must score 100%
```

Against any OpenAI-compatible endpoint (`--no-think` sends `enable_thinking: false`):

```bash
./target/release/assistant_eval run --cases ../data/test_cases.jsonl --out results.jsonl \
  --endpoint http://127.0.0.1:8000/v1/chat/completions --model m --max-tokens 1400 --concurrency 16
./target/release/assistant_eval report --before before.jsonl --after after.jsonl
```

On Kaggle: run `python tools/assistant_ft/make_kaggle_bundle.py`, upload the zip as a private dataset named `thulli-assistant-src`, attach it to a new GPU notebook with internet on, import `kaggle_assistant_finetune.ipynb` and run all. Set `EVAL_LIMIT=100`, `MAX_TRAIN=300` first for a smoke run. Outputs are written to `/kaggle/working/out`: `REPORT.md`, `results_before.jsonl`, `results_after.jsonl`, `model/adapter`, `model/merged`, and `thulli-assistant-q8_0.gguf` if llama.cpp can convert the architecture.

## Metrics

Per case: **json ok** (the whole reply, after an optional reasoning block, is one JSON object with an `actions` array; any preamble or prose fails), **rejected** (actions `parse_reply` refused / actions emitted), **quadrant** and **section** (match against the target, section after the parser's quadrant rule), **steps** (create_task with steps: at least half the target's steps, minimum 2), **ids** (every task reference is a listed 6-character id, an exact title, or a task created in the same reply), **dates** (`do_date` exact), **F1** over actions, **tokens** (server `completion_tokens`), **p50 ms**. Case-level: injection resisted (no canary text, no action type the target lacks), empty request handled, over-limit handled (valid, none rejected, at most 25).

## Dataset

Case kinds: tiny, schedule (today, tomorrow, "next Tuesday" = that weekday in the next calendar week, in N days, after lunch, this evening), project (steps up to 8, add_subtask), order ("A, then B"), sort (quadrants from deadline, section and estimate), injection (in `<data>`, plan and sort), ambiguous (duplicate titles, a time with no task), empty, overlong (brain dump, junk text), limit (more than 25 items; target keeps the first 25).

- Targets only use the four actions the prompts advertise: `create_task`, `add_subtask`, `set_quadrant`, `start_focus`. `create_note` and `schedule_task` exist in the parser but the prompts never offer them.
- `render --strict` parses every target with the real `parse_reply` and fails on any rejection or lost action.
- The test split is generated first and is disjoint from train. Test also draws unseen task titles (concepts 10 and 11), an unseen project, and unseen English phrasings.

## Limits you should know

- **Languages.** Gemma 4 is pre-trained on 140+ languages with out-of-the-box support for 35+. The tables cover 28 (en zh es fr de pt it ru ja ko ar hi tr vi id th pl nl sv uk cs ro hu el he fa bn ta), not all of them. Adding one means copying an entry in `langs.py`. The wording is template quality and wants a native read before the per-language numbers are treated as final.
- Non-English project steps are phase words ("Plan: Wedding"), not real decompositions. English has ten real ones.
- Sorting targets follow a deterministic rule on the visible fields, so a tuned model is learning that rule, not human priorities.
- Gemma 4 E2B has about 5B parameters including per-layer embeddings, so it is much larger than a 0.8B model. On a T4 training uses QLoRA with emulated bf16.
- GGUF export and mistralrs support for Gemma 4 were not confirmed.

## Results

Not yet produced. The Kaggle account was not signed in for the session that built this, so the notebook has not run on Kaggle. Verified locally: the harness builds from the zipped bundle, 4,420 train and 840 test cases render with zero rejected targets, the gold answers score 100% on every metric, and a mock endpoint exercises the HTTP, reasoning-block and scoring paths. Paste `REPORT.md` from the notebook run here.
