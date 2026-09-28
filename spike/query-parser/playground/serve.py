"""A local playground for the query box: type a sentence, see the model's word labels and the
decoded request. Runs the int8 file that would ship. Each parse is appended to
work/playground.jsonl, so good tries can become test sentences.

    speed/.venv/bin/python playground/serve.py [--model speed/work/onnx/ckpt-v2] [--port 8770]
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(ROOT))

import numpy as np  # noqa: E402
import onnxruntime as ort  # noqa: E402
from transformers import AutoTokenizer  # noqa: E402

from decode import decode  # noqa: E402
from schema import INTENTS, LABELS  # noqa: E402
from tagger import encode  # noqa: E402
from words import split  # noqa: E402


class Parser:
    def __init__(self, model_dir: Path):
        self.tok = AutoTokenizer.from_pretrained(str(model_dir))
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = 1
        self.sess = ort.InferenceSession(str(model_dir / "model.int8.onnx"), opts,
                                         providers=["CPUExecutionProvider"])

    def parse(self, text: str) -> dict:
        t0 = time.perf_counter()
        item = encode(self.tok, text)
        ids = np.array([item["input_ids"]], dtype=np.int64)
        il, tl = self.sess.run(["intent_logits", "tag_logits"],
                               {"input_ids": ids, "attention_mask": np.ones_like(ids)})
        t1 = time.perf_counter()
        ip = np.exp(il[0] - il[0].max())
        ip /= ip.sum()
        tp = np.exp(tl[0] - tl[0].max(-1, keepdims=True))
        tp /= tp.sum(-1, keepdims=True)
        row = tl[0].argmax(-1)
        out_words = []
        for (w, _, _), ti in zip(split(text), item["firsts"]):
            out_words.append({"w": w, "label": LABELS[row[ti]] if ti is not None else "O",
                              "p": round(float(tp[ti].max()), 3) if ti is not None else None})
        intent = INTENTS[int(il[0].argmax())]
        try:
            request = decode(text, intent, [x["label"] for x in out_words])
        except Exception as e:  # a decoder bug; show it instead of hiding it
            request = {"type": "none", "error": repr(e)}
        t2 = time.perf_counter()
        return {"text": text, "intent": intent, "intent_p": round(float(ip.max()), 3),
                "words": out_words, "request": request, "tokens": len(item["input_ids"]),
                "model_ms": round((t1 - t0) * 1000, 1), "decode_ms": round((t2 - t1) * 1000, 1)}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default=str(ROOT / "speed/work/onnx/ckpt-v2"))
    ap.add_argument("--port", type=int, default=8770)
    args = ap.parse_args()
    parser = Parser(Path(args.model))
    log = ROOT / "work" / "playground.jsonl"
    log.parent.mkdir(exist_ok=True)
    page = (HERE / "index.html").read_bytes()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send(self, body: bytes, ctype: str) -> None:
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            url = urlparse(self.path)
            if url.path == "/parse":
                q = parse_qs(url.query)
                text = q.get("q", [""])[0][:80]
                res = parser.parse(text) if text.strip() else {"text": "", "words": []}
                if text.strip() and q.get("log", ["0"])[0] == "1":
                    with log.open("a", encoding="utf-8") as f:
                        f.write(json.dumps(res, ensure_ascii=False) + "\n")
                self.send(json.dumps(res, ensure_ascii=False).encode(), "application/json")
            else:
                self.send(page, "text/html; charset=utf-8")

    print(f"playground on http://localhost:{args.port}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
