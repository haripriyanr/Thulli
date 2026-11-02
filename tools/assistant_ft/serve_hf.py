#!/usr/bin/env python3
"""Tiny OpenAI-compatible /v1/chat/completions server over a Hugging Face model.

It exists so the Rust eval harness can talk to the base model and the tuned model
the same way it would talk to any hosted endpoint. Requests are greedy and are
batched together, which keeps a 0.8B model fast on one Kaggle GPU.

    python serve_hf.py --model google/gemma-4-E2B-it --port 8000
"""
import argparse
import json
import queue
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import torch
from transformers import AutoModelForCausalLM, AutoTokenizer


def load(model_id, dtype):
    import transformers
    for name in ("AutoModelForCausalLM", "AutoModelForMultimodalLM", "AutoModelForImageTextToText"):
        cls = getattr(transformers, name, None)
        if cls is None:
            continue
        try:
            return cls.from_pretrained(model_id, torch_dtype=dtype, trust_remote_code=True)
        except (ValueError, KeyError):
            continue
    raise SystemExit(f"no transformers auto class could load {model_id}")


def pick_dtype(name):
    # bf16 everywhere by default: Gemma-family models can overflow in fp16. Older
    # GPUs (T4) run bf16 emulated, which is slower but correct.
    return {"bfloat16": torch.bfloat16, "float16": torch.float16, "float32": torch.float32}[name]


class Engine:
    def __init__(self, model_id, batch, wait_ms, dtype):
        self.tok = AutoTokenizer.from_pretrained(model_id, trust_remote_code=True)
        self.tok.padding_side = "left"
        if self.tok.pad_token is None:
            self.tok.pad_token = self.tok.eos_token
        self.model = load(model_id, pick_dtype(dtype)).to("cuda" if torch.cuda.is_available() else "cpu").eval()
        self.batch, self.wait = batch, wait_ms / 1000
        self.q = queue.Queue()
        threading.Thread(target=self.loop, daemon=True).start()

    def prompt(self, messages, kwargs):
        try:
            return self.tok.apply_chat_template(messages, tokenize=False, add_generation_prompt=True, **kwargs)
        except TypeError:
            return self.tok.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)

    def loop(self):
        while True:
            items = [self.q.get()]
            t0 = time.time()
            while len(items) < self.batch and time.time() - t0 < self.wait:
                try:
                    items.append(self.q.get(timeout=self.wait))
                except queue.Empty:
                    break
            try:
                self.run(items)
            except Exception as e:  # report to every waiting request, keep serving
                for it in items:
                    it["out"] = {"error": {"message": repr(e)}}
                    it["done"].set()

    @torch.no_grad()
    def run(self, items):
        texts = [self.prompt(i["messages"], i["kwargs"]) for i in items]
        enc = self.tok(texts, return_tensors="pt", padding=True, add_special_tokens=False).to(self.model.device)
        out = self.model.generate(**enc, max_new_tokens=max(i["max_tokens"] for i in items), do_sample=False,
                                  pad_token_id=self.tok.pad_token_id)
        new = out[:, enc["input_ids"].shape[1]:]
        gen_eos = self.model.generation_config.eos_token_id
        eos = {self.tok.eos_token_id, self.tok.pad_token_id}
        eos |= set(gen_eos if isinstance(gen_eos, (list, tuple)) else [gen_eos])
        eos.discard(None)
        for it, row in zip(items, new):
            ids = row.tolist()
            n = len(ids)
            for k, t in enumerate(ids):
                if t in eos:
                    n = k
                    break
            n = min(n, it["max_tokens"])
            text = self.tok.decode(ids[:n], skip_special_tokens=False)
            it["out"] = {"choices": [{"message": {"role": "assistant", "content": text}}],
                         "usage": {"completion_tokens": n, "prompt_tokens": int(enc["attention_mask"][0].sum())}}
            it["done"].set()

    def ask(self, messages, kwargs, max_tokens):
        it = {"messages": messages, "kwargs": kwargs, "max_tokens": max_tokens, "done": threading.Event()}
        self.q.put(it)
        it["done"].wait()
        return it["out"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", required=True)
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument("--batch", type=int, default=16)
    ap.add_argument("--wait-ms", type=int, default=40)
    ap.add_argument("--dtype", default="bfloat16", choices=["bfloat16", "float16", "float32"])
    a = ap.parse_args()
    eng = Engine(a.model, a.batch, a.wait_ms, a.dtype)

    class H(BaseHTTPRequestHandler):
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            out = eng.ask(body["messages"], body.get("chat_template_kwargs") or {}, int(body.get("max_tokens", 900)))
            data = json.dumps(out, ensure_ascii=False).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):  # readiness probe
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")

        def log_message(self, *args):
            pass

    print("ready", flush=True)
    ThreadingHTTPServer(("127.0.0.1", a.port), H).serve_forever()


if __name__ == "__main__":
    main()
