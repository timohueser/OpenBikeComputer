"""Fine-tunes the joint tagger on generated data.

    speed/.venv/bin/python train.py --base speed/work/cut-50k \
        --train data/train/en.jsonl data/train/de.jsonl --dev data/dev/en.jsonl data/dev/de.jsonl \
        --out work/ckpt-en-de
"""

from __future__ import annotations

import argparse
import json
import math
import random
import time
from pathlib import Path

import torch
from torch import nn

from tagger import INTENTS, LABELS, collate, encode, joint_tagger, predict


def read(paths: list[str]) -> list[dict]:
    rows = []
    for p in paths:
        rows += [json.loads(line) for line in Path(p).read_text(encoding="utf-8").splitlines() if line]
    return rows


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True, help="encoder checkpoint (a cut mmBERT-small)")
    ap.add_argument("--train", nargs="+", required=True)
    ap.add_argument("--dev", nargs="*", default=[])
    ap.add_argument("--out", required=True)
    ap.add_argument("--epochs", type=int, default=3)
    ap.add_argument("--batch", type=int, default=32)
    ap.add_argument("--lr", type=float, default=5e-5)
    ap.add_argument("--head-lr", type=float, default=5e-4)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--device", default="mps" if torch.backends.mps.is_available() else "cpu")
    args = ap.parse_args()

    random.seed(args.seed)
    torch.manual_seed(args.seed)
    from transformers import AutoTokenizer

    tok = AutoTokenizer.from_pretrained(args.base)
    model = joint_tagger(args.base, seed=args.seed).to(args.device).train()

    def items(rows):
        out = []
        for r in rows:
            it = encode(tok, r["text"], r["labels"])
            it["intent"] = INTENTS.index(r["intent"])
            out.append(it)
        return out

    train = items(read(args.train))
    dev_rows = read(args.dev)
    print(f"train {len(train)}  dev {len(dev_rows)}  labels {len(LABELS)}", flush=True)

    heads = [p for n, p in model.named_parameters() if not n.startswith("encoder.")]
    enc = [p for n, p in model.named_parameters() if n.startswith("encoder.")]
    opt = torch.optim.AdamW([{"params": enc, "lr": args.lr}, {"params": heads, "lr": args.head_lr}],
                            weight_decay=0.01)
    steps = args.epochs * math.ceil(len(train) / args.batch)
    warm = max(1, steps // 16)
    sched = torch.optim.lr_scheduler.LambdaLR(
        opt, lambda s: min(1.0, (s + 1) / warm) * max(0.0, (steps - s) / max(1, steps - warm)))
    ce = nn.CrossEntropyLoss(ignore_index=-100)

    step = 0
    for epoch in range(args.epochs):
        random.shuffle(train)
        t0, total = time.time(), 0.0
        for b in range(0, len(train), args.batch):
            x = collate(train[b:b + args.batch], tok.pad_token_id)
            il, tl = model(x["input_ids"].to(args.device), x["attention_mask"].to(args.device))
            loss = ce(il, x["intent"].to(args.device)) + ce(
                tl.reshape(-1, tl.shape[-1]), x["tags"].to(args.device).reshape(-1))
            opt.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            opt.step()
            sched.step()
            total += loss.item()
            step += 1
            if step % 200 == 0:
                print(f"epoch {epoch} step {step}/{steps} loss {total / 200:.4f} "
                      f"{time.time() - t0:.0f}s", flush=True)
                total = 0.0
        if dev_rows:
            model.eval()
            preds = predict(tok, model, [r["text"] for r in dev_rows], args.device)
            model.train()
            ia = sum(p[0] == r["intent"] for p, r in zip(preds, dev_rows)) / len(dev_rows)
            wa = sum(p[1] == r["labels"] for p, r in zip(preds, dev_rows)) / len(dev_rows)
            print(f"epoch {epoch} dev intent {ia:.4f}  all-labels-right {wa:.4f}", flush=True)

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    model.encoder.save_pretrained(out)
    tok.save_pretrained(out)
    from safetensors.torch import save_file

    save_file({k: v.detach().cpu().contiguous() for k, v in model.state_dict().items()
               if not k.startswith("encoder.")}, str(out / "heads.safetensors"))
    (out / "labels.json").write_text(json.dumps({"intents": INTENTS, "labels": LABELS}))
    print(f"saved {out}")


if __name__ == "__main__":
    main()
