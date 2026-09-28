"""Time one parse (tokenize + model) natively with onnxruntime CPU and PyTorch eager.

    .venv/bin/python bench_native.py --onnx work/onnx/cut-50k/model.int8.onnx work/onnx/cut-50k/model.onnx \
        --torch work/cut-50k --threads 1 0

Every configuration runs in a fresh process so that load time, first parse and resident
memory are its own. Threads 0 means the runtime default. Sentences: queries.tsv, en/de/fr/it.
Writes work/bench_native.json and prints one row per configuration.
"""
import argparse
import json
import statistics
import subprocess
import sys
import time
from pathlib import Path

from common import QUERIES, SPEED, WORK, load_sentences

LANGS = ("en", "de", "fr", "it")


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(p / 100 * (len(xs) - 1))))]


def one(kind, path, threads, runs):
    import numpy as np
    import psutil
    from tokenizers import Tokenizer

    sentences = [s for _, s in load_sentences(QUERIES, LANGS)]
    proc = psutil.Process()
    rss0 = proc.memory_info().rss
    t0 = time.perf_counter()
    if kind == "ort":
        import onnxruntime as ort

        tok = Tokenizer.from_file(str(Path(path).parent / "tokenizer.json"))
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = threads
        opts.inter_op_num_threads = 1
        sess = ort.InferenceSession(path, opts, providers=["CPUExecutionProvider"])

        def infer(ids):
            a = np.array([ids], dtype=np.int64)
            return sess.run(None, {"input_ids": a, "attention_mask": np.ones_like(a)})
    else:
        import torch

        from common import joint_tagger

        if threads:
            torch.set_num_threads(threads)
        model = joint_tagger(SPEED / path)
        tok = Tokenizer.from_file(str(SPEED / path / "tokenizer.json"))

        def infer(ids):
            with torch.inference_mode():
                a = torch.tensor([ids])
                return model(a, torch.ones_like(a))
    load_ms = (time.perf_counter() - t0) * 1e3

    def parse(s):
        t = time.perf_counter()
        ids = tok.encode(s).ids
        t_tok = time.perf_counter()
        infer(ids)
        return (time.perf_counter() - t) * 1e3, (t_tok - t) * 1e3

    first_ms, _ = parse(sentences[0])
    times, tok_times = [], []
    for i in range(runs):
        total, tk = parse(sentences[(i + 1) % len(sentences)])
        times.append(total)
        tok_times.append(tk)
    rss = proc.memory_info().rss
    return {"runtime": kind, "model": str(path), "threads": threads, "load_ms": load_ms, "first_ms": first_ms,
            "median_ms": statistics.median(times), "p95_ms": pct(times, 95), "tokenize_median_ms": statistics.median(tok_times),
            "runs": runs, "rss_mb": rss / 1e6, "rss_growth_mb": (rss - rss0) / 1e6}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--onnx", nargs="*", default=[])
    ap.add_argument("--torch", nargs="*", default=[], help="checkpoint dirs for the PyTorch eager reference")
    ap.add_argument("--threads", nargs="+", type=int, default=[1, 0])
    ap.add_argument("--runs", type=int, default=300)
    ap.add_argument("--one", nargs=3, help=argparse.SUPPRESS)
    args = ap.parse_args()
    if args.one:
        print(json.dumps(one(args.one[0], args.one[1], int(args.one[2]), args.runs)))
        return

    from tokenizers import Tokenizer

    first = args.onnx[0] if args.onnx else str(Path(args.torch[0]) / "model.onnx")
    tok = Tokenizer.from_file(str(SPEED / Path(first).parent / "tokenizer.json"))
    lens = [len(tok.encode(s).ids) for _, s in load_sentences(QUERIES, LANGS)]
    print(f"tokens per sentence (with bos/eos): n={len(lens)} min={min(lens)} median={statistics.median(lens)} "
          f"p95={pct(lens, 95)} max={max(lens)}")
    rows = []
    configs = [("ort", str(SPEED / p), t) for p in args.onnx for t in args.threads]
    configs += [("torch", p, t) for p in args.torch for t in args.threads]
    for kind, path, threads in configs:
        out = subprocess.run([sys.executable, __file__, "--runs", str(args.runs), "--one", kind, path, str(threads)],
                             capture_output=True, text=True, cwd=SPEED)
        if out.returncode:
            print(kind, path, threads, "FAILED", out.stderr[-500:])
            continue
        r = json.loads(out.stdout.strip().splitlines()[-1])
        rows.append(r)
        name = Path(path).name if kind == "ort" else "torch-eager"
        print(f"{kind:5} {name:20} threads={threads}: first {r['first_ms']:.1f} ms, median {r['median_ms']:.1f} ms, "
              f"p95 {r['p95_ms']:.1f} ms, load {r['load_ms']:.0f} ms, rss {r['rss_mb']:.0f} MB "
              f"(+{r['rss_growth_mb']:.0f}), tokenize {r['tokenize_median_ms']:.3f} ms")
    (WORK / "bench_native.json").write_text(json.dumps({"token_counts": lens, "rows": rows}, indent=1))


if __name__ == "__main__":
    main()
