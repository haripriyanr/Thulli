#!/usr/bin/env python3
"""LoRA (or QLoRA) fine-tune for the Thulli assistant, then merge the adapter.

Input is `train_rendered.jsonl` from `assistant_eval render`: the exact prompt the
app builds plus the target reply. Only the reply is trained on, so the model
learns to answer with the JSON object straight away: no reasoning block, no
preamble, no code fence.

The prompt is rendered with the model's own chat template (thinking off, which is
Gemma 4's default). The completion is whatever the template appends for an
assistant turn holding the target, so the end-of-turn token is always the one
the model and the on-device runtime expect. Only the text layers get adapters;
the image and audio towers are left alone.
"""
import argparse
import json
import os
import random

import torch
from peft import LoraConfig, get_peft_model
from transformers import AutoTokenizer, Trainer, TrainingArguments


def load(model_id, dtype, four_bit):
    import transformers
    kw = dict(trust_remote_code=True)
    if four_bit:
        from transformers import BitsAndBytesConfig
        kw["quantization_config"] = BitsAndBytesConfig(load_in_4bit=True, bnb_4bit_quant_type="nf4",
                                                       bnb_4bit_compute_dtype=dtype, bnb_4bit_use_double_quant=True)
        kw["device_map"] = {"": 0}
    kw["torch_dtype"] = dtype
    for name in ("AutoModelForCausalLM", "AutoModelForMultimodalLM", "AutoModelForImageTextToText"):
        cls = getattr(transformers, name, None)
        if cls is None:
            continue
        try:
            return cls.from_pretrained(model_id, **kw)
        except (ValueError, KeyError):
            continue
    raise SystemExit(f"no transformers auto class could load {model_id}")


def lora_targets(model):
    """Linear layers of the language model only, by regex; all-linear if names don't say."""
    names = [n for n, _ in model.named_modules()]
    if any("language_model" in n for n in names):
        return r".*language_model.*\.(q_proj|k_proj|v_proj|o_proj|gate_proj|up_proj|down_proj)"
    return "all-linear"


def split_turn(tok, messages, target):
    """(prompt, completion) text for one example, from the chat template itself."""
    prompt = tok.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
    full = tok.apply_chat_template(messages + [{"role": "assistant", "content": target}], tokenize=False)
    if full.startswith(prompt) and target in full[len(prompt):]:
        return prompt, full[len(prompt):]
    return prompt, target + (tok.eos_token or "")


def build_examples(path, tok, max_len):
    out, dropped = [], 0
    for line in open(path, encoding="utf-8"):
        r = json.loads(line)
        prompt, completion = split_turn(tok, r["messages"], r["target"])
        p_ids = tok(prompt, add_special_tokens=False)["input_ids"]
        c_ids = tok(completion, add_special_tokens=False)["input_ids"]
        if len(p_ids) + len(c_ids) > max_len:
            dropped += 1
            continue
        out.append({"input_ids": p_ids + c_ids, "labels": [-100] * len(p_ids) + c_ids})
    print(f"{len(out)} training examples, {dropped} dropped for length > {max_len}")
    if out:
        print("example completion tail:", repr(tok.decode(out[0]["input_ids"][-12:])))
    return out


class Collate:
    def __init__(self, pad):
        self.pad = pad

    def __call__(self, batch):
        n = max(len(b["input_ids"]) for b in batch)
        ids = torch.full((len(batch), n), self.pad, dtype=torch.long)
        lab = torch.full((len(batch), n), -100, dtype=torch.long)
        att = torch.zeros((len(batch), n), dtype=torch.long)
        for i, b in enumerate(batch):
            k = len(b["input_ids"])
            ids[i, :k] = torch.tensor(b["input_ids"])
            lab[i, :k] = torch.tensor(b["labels"])
            att[i, :k] = 1
        return {"input_ids": ids, "labels": lab, "attention_mask": att}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="google/gemma-4-E2B-it")
    ap.add_argument("--data", required=True)
    ap.add_argument("--out", required=True, help="directory for the adapter and the merged model")
    ap.add_argument("--epochs", type=float, default=2.0)
    ap.add_argument("--lr", type=float, default=2e-4)
    ap.add_argument("--rank", type=int, default=32)
    ap.add_argument("--batch", type=int, default=4)
    ap.add_argument("--accum", type=int, default=4)
    ap.add_argument("--max-len", type=int, default=3072)
    ap.add_argument("--max-examples", type=int, default=0)
    ap.add_argument("--qlora", default="auto", choices=["auto", "yes", "no"], help="4-bit base weights; auto = yes unless the GPU has bf16 and 22GB+")
    ap.add_argument("--seed", type=int, default=7)
    a = ap.parse_args()

    tok = AutoTokenizer.from_pretrained(a.model, trust_remote_code=True)
    if tok.pad_token is None:
        tok.pad_token = tok.eos_token
    data = build_examples(a.data, tok, a.max_len)
    random.Random(a.seed).shuffle(data)
    if a.max_examples:
        data = data[: a.max_examples]

    cuda = torch.cuda.is_available()
    bf16 = cuda and torch.cuda.get_device_capability()[0] >= 8
    vram = torch.cuda.get_device_properties(0).total_memory / 2**30 if cuda else 0
    qlora = a.qlora == "yes" or (a.qlora == "auto" and not (bf16 and vram >= 22))
    print(f"bf16={bf16} vram={vram:.0f}GB qlora={qlora}")
    # Weights/compute in bf16 (Gemma can overflow in fp16); GPUs without native bf16 emulate it.
    model = load(a.model, torch.bfloat16, qlora)
    if qlora:
        from peft import prepare_model_for_kbit_training
        model = prepare_model_for_kbit_training(model)
    model.config.use_cache = False
    cfg = LoraConfig(r=a.rank, lora_alpha=a.rank * 2, lora_dropout=0.05, target_modules=lora_targets(model), task_type="CAUSAL_LM")
    model = get_peft_model(model, cfg)
    model.print_trainable_parameters()

    args = TrainingArguments(
        output_dir=os.path.join(a.out, "trainer"), num_train_epochs=a.epochs, per_device_train_batch_size=a.batch,
        gradient_accumulation_steps=a.accum, learning_rate=a.lr, lr_scheduler_type="cosine", warmup_ratio=0.03,
        logging_steps=10, save_strategy="no", bf16=bf16, fp16=False, gradient_checkpointing=True,
        gradient_checkpointing_kwargs={"use_reentrant": False}, report_to="none", remove_unused_columns=False,
        dataloader_num_workers=2, seed=a.seed,
    )
    if hasattr(model, "enable_input_require_grads"):
        model.enable_input_require_grads()
    Trainer(model=model, args=args, train_dataset=data, data_collator=Collate(tok.pad_token_id)).train()

    adapter = os.path.join(a.out, "adapter")
    model.save_pretrained(adapter)
    tok.save_pretrained(adapter)
    del model
    torch.cuda.empty_cache()

    # Merge into a full-precision copy of the base weights (not the 4-bit ones).
    from peft import PeftModel
    base = load(a.model, torch.bfloat16, False)
    merged = PeftModel.from_pretrained(base, adapter).merge_and_unload()
    merged_dir = os.path.join(a.out, "merged")
    merged.save_pretrained(merged_dir, safe_serialization=True)
    tok.save_pretrained(merged_dir)
    print("saved", adapter, "and", merged_dir)


if __name__ == "__main__":
    main()
