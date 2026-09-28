"""The joint tagger at training and evaluation time: word-to-token alignment, batching and
prediction. A checkpoint directory holds the encoder, the tokenizer and `heads.safetensors`
(keys `intent.*`, `tags.*`), which is the layout `speed/common.py` exports and benches."""

from __future__ import annotations

import sys
from pathlib import Path

import torch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "speed"))

from common import joint_tagger  # noqa: E402
from schema import INTENTS, LABELS  # noqa: E402
from words import split  # noqa: E402

MAX_LEN = 64


def first_tokens(offsets: list[tuple[int, int]], text: str) -> list[int | None]:
    """For each word of `text`, the index of the first token that overlaps it."""
    out: list[int | None] = []
    for _, ws, we in split(text):
        hit = None
        for ti, (s, e) in enumerate(offsets):
            if s < e and max(s, ws) < min(e, we):
                hit = ti
                break
        out.append(hit)
    return out


def encode(tok, text: str, labels: list[str] | None = None) -> dict:
    enc = tok(text, return_offsets_mapping=True, truncation=True, max_length=MAX_LEN)
    firsts = first_tokens(enc["offset_mapping"], text)
    item = {"input_ids": enc["input_ids"], "firsts": firsts}
    if labels is not None:
        assert len(labels) == len(firsts), (text, labels)
        tags = [-100] * len(enc["input_ids"])
        for lab, ti in zip(labels, firsts):
            if ti is not None and tags[ti] == -100:
                tags[ti] = LABELS.index(lab)
        item["tags"] = tags
    return item


def collate(items: list[dict], pad_id: int) -> dict:
    n = max(len(i["input_ids"]) for i in items)
    ids = torch.full((len(items), n), pad_id, dtype=torch.long)
    mask = torch.zeros((len(items), n), dtype=torch.long)
    tags = torch.full((len(items), n), -100, dtype=torch.long)
    for r, i in enumerate(items):
        k = len(i["input_ids"])
        ids[r, :k] = torch.tensor(i["input_ids"])
        mask[r, :k] = 1
        if "tags" in i:
            tags[r, :k] = torch.tensor(i["tags"])
    out = {"input_ids": ids, "attention_mask": mask, "tags": tags}
    if "intent" in items[0]:
        out["intent"] = torch.tensor([i["intent"] for i in items])
    return out


def load(checkpoint: str | Path, device: str = "cpu"):
    from transformers import AutoTokenizer

    tok = AutoTokenizer.from_pretrained(str(checkpoint))
    model = joint_tagger(checkpoint).to(device).eval()
    return tok, model


@torch.no_grad()
def predict(tok, model, texts: list[str], device: str = "cpu", batch: int = 64):
    """(intent, word labels, confidence) for each text. The confidence is the lowest softmax
    probability of the sentence label and of any word label."""
    out = []
    for b in range(0, len(texts), batch):
        chunk = texts[b:b + batch]
        items = [encode(tok, t) for t in chunk]
        x = collate(items, tok.pad_token_id)
        il, tl = model(x["input_ids"].to(device), x["attention_mask"].to(device))
        ip, intents = il.softmax(-1).max(-1)
        tp, tags = tl.softmax(-1).max(-1)
        for r, item in enumerate(items):
            firsts = [ti for ti in item["firsts"] if ti is not None]
            labels = [LABELS[tags[r, ti]] if ti is not None else "O" for ti in item["firsts"]]
            conf = min([ip[r].item()] + [tp[r, ti].item() for ti in firsts])
            out.append((INTENTS[intents[r]], labels, conf))
    return out
