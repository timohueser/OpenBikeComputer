"""Print the config facts, tokenizer type and parameter split of a checkpoint.

    .venv/bin/python inspect_model.py [--model jhu-clsp/mmBERT-small | work/cut-50k]
"""
import argparse
import json

from common import BASE_MODEL, load_encoder, model_dir

ap = argparse.ArgumentParser()
ap.add_argument("--model", default=BASE_MODEL)
args = ap.parse_args()

d = model_dir(args.model)
cfg = json.loads((d / "config.json").read_text())
keys = ["num_hidden_layers", "hidden_size", "intermediate_size", "num_attention_heads", "vocab_size",
        "max_position_embeddings", "local_attention", "global_attn_every_n_layers", "hidden_activation"]
print({k: cfg.get(k) for k in keys})
tok = json.loads((d / "tokenizer.json").read_text())
m = tok["model"]
print(f"tokenizer: {m['type']} byte_fallback={m.get('byte_fallback')} vocab={len(m['vocab'])} "
      f"merges={len(m['merges'])} normalizer={tok['normalizer']['type']} pre={tok['pre_tokenizer']['type']}")

enc = load_encoder(d)
emb = enc.embeddings.tok_embeddings.weight.numel()
total = sum(p.numel() for p in enc.parameters())
print(f"encoder params: total={total/1e6:.2f}M embedding={emb/1e6:.2f}M rest={(total-emb)/1e6:.2f}M")
