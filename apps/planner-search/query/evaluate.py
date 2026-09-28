"""Scores a parser on the held-out test set (testset/*.jsonl).

    python evaluate.py rules
    speed/.venv/bin/python evaluate.py model:work/ckpt-en-de
    speed/.venv/bin/python evaluate.py onnx:work/ckpt-en-de/model.int8.onnx --tokenizer work/ckpt-en-de

Accuracy is an exact match of `schema.canonical` on in-scope lines. A silent error is a wrong
request that the box would show as understood: not `none` and nothing ignored. An out-of-scope
line is handled when the parser gives `none` or ignores some words.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from schema import canonical, fold_name, same, validate  # noqa: E402


def load_testset(paths: list[str] | None) -> list[dict]:
    files = [Path(p) for p in paths] if paths else sorted((HERE / "testset").glob("*.jsonl"))
    rows = []
    for f in files:
        rows += [json.loads(line) for line in f.read_text(encoding="utf-8").splitlines() if line]
    return rows


def rules_parser():
    from rules import parse

    return lambda texts: [parse(t) for t in texts]


def gated(request: dict, conf: float, gate: float) -> dict:
    """Below the gate the box shows the request as not sure: every word stays visible."""
    if conf >= gate or request["type"] == "none" or request.get("ignored"):
        return request
    return {**request, "ignored": ["(not sure)"]}


def model_parser(checkpoint: str, gate: float):
    from decode import decode
    from tagger import load, predict

    tok, model = load(checkpoint)
    return lambda texts: [gated(decode(t, i, labs), c, gate)
                          for t, (i, labs, c) in zip(texts, predict(tok, model, texts))]


def onnx_parser(path: str, tokenizer: str, gate: float):
    import numpy as np
    import onnxruntime as ort
    from transformers import AutoTokenizer

    from decode import decode
    from schema import INTENTS, LABELS
    from tagger import encode

    tok = AutoTokenizer.from_pretrained(tokenizer)
    sess = ort.InferenceSession(path, providers=["CPUExecutionProvider"])

    def run(texts):
        out = []
        for t in texts:
            item = encode(tok, t)
            ids = np.array([item["input_ids"]], dtype=np.int64)
            il, tl = sess.run(["intent_logits", "tag_logits"],
                              {"input_ids": ids, "attention_mask": np.ones_like(ids)})
            ip = np.exp(il[0] - il[0].max()); ip /= ip.sum()
            tp = np.exp(tl[0] - tl[0].max(-1, keepdims=True)); tp /= tp.sum(-1, keepdims=True)
            row = tl[0].argmax(-1)
            firsts = [ti for ti in item["firsts"] if ti is not None]
            labels = [LABELS[row[ti]] if ti is not None else "O" for ti in item["firsts"]]
            conf = min([ip.max()] + [tp[ti].max() for ti in firsts])
            out.append(gated(decode(t, INTENTS[int(il[0].argmax())], labels), conf, gate))
        return out

    return run


def safe(parse, texts: list[str]) -> list[dict]:
    """Parses one text at a time when the batch fails, so one crash costs one line."""
    try:
        preds = parse(texts)
    except Exception:
        preds = []
        for t in texts:
            try:
                preds.append(parse([t])[0])
            except Exception as e:  # a crash is a parser bug; score it as not understood
                preds.append({"type": "none", "error": repr(e)})
    out = []
    for p in preds:
        try:
            validate({k: v for k, v in p.items() if k != "error"})
            out.append(p)
        except Exception as e:
            out.append({"type": "none", "error": f"invalid: {e}"})
    return out


def as_search(r: dict, text: str) -> dict:
    """A `place` whose name is the whole text is the place search that `none` gives."""
    if (r["type"] == "place" and "near" not in r and not r.get("ignored")
            and fold_name(r["name"]) == fold_name(text)):
        return {"type": "none"}
    return r


def score(rows: list[dict], preds: list[dict]) -> dict:
    by = defaultdict(lambda: [0, 0])
    silent = inscope = oos = oos_ok = oos_silent = crashes = 0
    errors = []
    for r, p in zip(rows, preds):
        crashes += "error" in p
        p = as_search(p, r["text"])
        r = {**r, "request": as_search(r["request"], r["text"])}
        understood = p["type"] != "none" and not p.get("ignored")
        if r["scope"] == "in":
            inscope += 1
            ok = same(p, r["request"])
            for key in (f"lang:{r['lang']}", f"type:{r['request']['type']}", "all"):
                by[key][0] += ok
                by[key][1] += 1
            if not ok:
                silent += understood
                errors.append({"id": r["id"], "text": r["text"], "silent": understood,
                               "gold": canonical(r["request"]), "pred": p})
        else:
            oos += 1
            oos_ok += not understood
            oos_silent += understood
            if understood:
                errors.append({"id": r["id"], "text": r["text"], "silent": True, "oos": True,
                               "gold": canonical(r["request"]), "pred": p})
    return {"acc": {k: (v[0], v[1]) for k, v in sorted(by.items())}, "silent": (silent, inscope),
            "oos_handled": (oos_ok, oos), "oos_silent": (oos_silent, oos), "crashes": crashes,
            "errors": errors}


def pct(a: int, b: int) -> str:
    return f"{100 * a / b:5.1f} % ({a}/{b})" if b else "-"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("parser", help="runtime:<directory> | rules | model:<checkpoint> | onnx:<file>")
    ap.add_argument("--tokenizer")
    ap.add_argument("--files", nargs="*")
    ap.add_argument("--errors", help="write the errors as JSON lines here")
    ap.add_argument("--gate", type=float, default=0.0, help="confidence below which a request is shown as not sure")
    args = ap.parse_args()

    if args.parser.startswith("runtime:"):
        from runtime import Parser
        runtime = Parser(Path(args.parser[8:]))
        parse = lambda texts: [runtime.parse(t)["request"] for t in texts]
    elif args.parser == "rules":
        parse = rules_parser()
    elif args.parser.startswith("model:"):
        parse = model_parser(args.parser[6:], args.gate)
    elif args.parser.startswith("onnx:"):
        parse = onnx_parser(args.parser[5:], args.tokenizer or str(Path(args.parser[5:]).parent), args.gate)
    else:
        sys.exit(f"unknown parser {args.parser}")

    rows = load_testset(args.files)
    s = score(rows, safe(parse, [r["text"] for r in rows]))
    print(f"parser {args.parser}")
    for k, (a, b) in s["acc"].items():
        print(f"  accuracy {k:22s} {pct(a, b)}")
    print(f"  silent errors (in scope)       {pct(*s['silent'])}")
    print(f"  out of scope handled           {pct(*s['oos_handled'])}")
    print(f"  out of scope silent            {pct(*s['oos_silent'])}")
    print(f"  crashes or invalid requests    {s['crashes']}")
    if args.errors:
        Path(args.errors).write_text(
            "".join(json.dumps(e, ensure_ascii=False) + "\n" for e in s["errors"]), encoding="utf-8")

    if s["crashes"]:
        sys.exit(1)


if __name__ == "__main__":
    main()
