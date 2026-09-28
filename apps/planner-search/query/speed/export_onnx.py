"""Export the joint tagger to ONNX (fp32, int8 per-tensor, int8 per-channel) and check int8 against fp32.

    .venv/bin/python export_onnx.py --checkpoint work/cut-50k --out work/onnx/cut-50k

The checkpoint is a cut encoder directory (cut_vocab.py) with an optional heads.safetensors
(keys intent.weight, intent.bias, tags.weight, tags.bias); without it the heads are random.
The output directory gets model.onnx, model.int8.onnx (per-tensor weights), model.int8pc.onnx
(per-channel weights), the tokenizer files,
expected_ids.json (the bench sentences with their token ids, for the web and Rust benches) and
export_report.json (sizes raw/gzip/brotli, int8-vs-fp32 differences).
"""
import argparse
import gzip
import json
import shutil
from pathlib import Path

import brotli
import numpy as np
import onnx
import onnxruntime as ort
import torch
from onnxruntime.quantization import QuantType, quantize_dynamic
from tokenizers import Tokenizer

from common import HELDOUT, QUERIES, SPEED, joint_tagger, load_sentences, model_dir


def sizes(path: Path):
    raw = path.read_bytes()
    quality = 11 if len(raw) < 150e6 else 9  # brotli 11 on the fp32 file takes over 20 minutes
    return {"raw_mb": len(raw) / 1e6, "gzip9_mb": len(gzip.compress(raw, 9)) / 1e6,
            f"brotli{quality}_mb": len(brotli.compress(raw, quality=quality)) / 1e6}


OUTPUTS = ["intent_logits", "tag_logits"]


def feed(ids):
    return {"input_ids": np.array([ids], dtype=np.int64), "attention_mask": np.ones((1, len(ids)), dtype=np.int64)}


def outlier_matmuls(fp32: Path, feeds, limit: float):
    """Weight MatMuls whose input activation exceeds `limit` on the sample.

    Dynamic int8 quantises activations per tensor. mmBERT has a few massive activations
    (thousands, next to values below 100), which leave the rest of the tensor with no
    levels, so those MatMuls stay fp32.
    """
    m = onnx.load(fp32)
    weights = {i.name for i in m.graph.initializer}
    nodes = [n for n in m.graph.node if n.op_type in ("MatMul", "Gemm") and n.input[1] in weights]
    names = sorted({n.input[0] for n in nodes})
    m.graph.output.extend(onnx.helper.make_tensor_value_info(x, onnx.TensorProto.FLOAT, None) for x in names)
    sess = ort.InferenceSession(m.SerializeToString(), providers=["CPUExecutionProvider"])
    peak = dict.fromkeys(names, 0.0)
    for f in feeds:
        for x, v in zip(names, sess.run(names, f)):
            peak[x] = max(peak[x], float(np.abs(v).max()))
    return [n.name for n in nodes if peak[n.input[0]] > limit]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--checkpoint", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--opset", type=int, default=18)
    ap.add_argument("--outlier-limit", type=float, default=500.0,
                    help="MatMuls with a larger input activation stay fp32 (mmBERT-small: two MatMuls at 4600 and 9400, the rest below 100); inf disables")
    args = ap.parse_args()
    ckpt = model_dir(str(SPEED / args.checkpoint) if not Path(args.checkpoint).is_absolute() else args.checkpoint)
    out = SPEED / args.out
    out.mkdir(parents=True, exist_ok=True)
    for f in ["tokenizer.json", "tokenizer_config.json"]:
        if (ckpt / f).exists():
            shutil.copy(ckpt / f, out / f)
    tok = Tokenizer.from_file(str(ckpt / "tokenizer.json"))
    expected = [{"lang": l, "text": s, "ids": tok.encode(s).ids} for l, s in load_sentences(QUERIES, ("en", "de", "fr", "it"))]
    (out / "expected_ids.json").write_text(json.dumps(expected, ensure_ascii=False))

    model = joint_tagger(ckpt)
    ids = torch.tensor([tok.encode("campsites end of day 4").ids])
    fp32 = out / "model.onnx"
    seq = torch.export.Dim("seq", min=2, max=512)
    torch.onnx.export(
        model, (ids, torch.ones_like(ids)), str(fp32), dynamo=True, opset_version=args.opset,
        input_names=["input_ids", "attention_mask"], output_names=["intent_logits", "tag_logits"],
        dynamic_shapes={"input_ids": {1: seq}, "attention_mask": {1: seq}}, external_data=False)
    # The quantizer turns Gemm into MatMul with a transposed weight; stale value_info then breaks shape inference.
    graph = onnx.load(fp32)
    del graph.graph.value_info[:]
    onnx.save(graph, fp32)

    sentences = [s for _, s in load_sentences(QUERIES)] + [s for _, s in load_sentences(HELDOUT)]
    sentences = sentences[:200]
    feeds = [feed(tok.encode(s).ids) for s in sentences]
    keep_fp32 = outlier_matmuls(fp32, feeds, args.outlier_limit)
    variants = {"fp32": fp32}
    for name, per_channel in [("int8", False), ("int8pc", True)]:
        variants[name] = out / f"model.{name}.onnx"
        quantize_dynamic(fp32, variants[name], weight_type=QuantType.QInt8, per_channel=per_channel,
                         nodes_to_exclude=keep_fp32)

    sessions = {k: ort.InferenceSession(str(p), providers=["CPUExecutionProvider"]) for k, p in variants.items()}
    report = {"sizes": {k: sizes(p) for k, p in variants.items()}, "fp32_matmuls": keep_fp32,
              "sentences": len(sentences), "check": {}}

    with torch.no_grad():
        torch_diff = max(
            max(np.abs(t.numpy() - o).max() for t, o in zip(model(*map(torch.from_numpy, f.values())), sessions["fp32"].run(OUTPUTS, f)))
            for f in feeds[:20])
    report["onnx_fp32_vs_torch_max_abs_diff"] = float(torch_diff)

    ref = [sessions["fp32"].run(OUTPUTS, f) for f in feeds]
    for name in ["int8", "int8pc"]:
        di = dt = 0.0
        agree_i = agree_t = n_t = 0
        cos = []
        for f, (ri, rt) in zip(feeds, ref):
            qi, qt = sessions[name].run(OUTPUTS, f)
            di, dt = max(di, np.abs(qi - ri).max()), max(dt, np.abs(qt - rt).max())
            agree_i += int(qi.argmax() == ri.argmax())
            a, b = rt[0, 1:-1], qt[0, 1:-1]  # content tokens only: <bos> and <eos> carry no tag
            agree_t += int((a.argmax(-1) == b.argmax(-1)).sum())
            n_t += len(a)
            cos += list((a * b).sum(-1) / np.linalg.norm(a, axis=-1) / np.linalg.norm(b, axis=-1))
        report["check"][name] = {"intent_max_abs_diff": float(di), "tag_max_abs_diff": float(dt),
                                 "intent_argmax_agree": f"{agree_i}/{len(feeds)}",
                                 "content_tag_argmax_agree": f"{agree_t}/{n_t}",
                                 "content_tag_logit_cos_mean": float(np.mean(cos)),
                                 "content_tag_logit_cos_p1": float(np.percentile(cos, 1))}
    (out / "export_report.json").write_text(json.dumps(report, indent=1))
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
