# Query box parser prototype

The prototype of the route planner's query box ([#2238](https://github.com/timohueser/OpenBikeComputer/issues/2238)).
One sentence of at most 80 characters goes in; one typed request comes out. A fine-tuned
mmBERT-small tags the words, and deterministic code decodes the tags. The results, strengths and
limits are in the archive issue linked from #2238. This file says how to rebuild and run it.

This branch is never merged. The code is kept by the tag `spike/query-parser-v2`. The weights
and the data are assets of the release with the same name.

## Layout

| Path | What it is |
| --- | --- |
| `schema.py` | Request language v0: request types, building blocks, labels, `validate()`, `canonical()` |
| `words.py` | The word split that the word labels refer to |
| `lexicon.py`, `lexicon/` | Word lists (kinds from the iD tagging schema 6.19.2, ISC; the rest hand-written) and the matcher |
| `decode.py` | Tags → request. Half of the parser; it ships with the model |
| `rules.py` | Rules-only baseline (for comparison only) |
| `templates/`, `gen/` | Per-language templates and the seeded data generator |
| `train.py`, `tagger.py` | Joint tagger training (sentence head + BIO token head) |
| `evaluate.py`, `testset/` | Scoring on the 350 hand-written test sentences |
| `speed/` | Vocabulary cut, ONNX export (int8 fix), native, browser and Rust benches |
| `playground/` | Local web page: type a sentence, see the chips |
| `test_decode.py` | Decoder tests (245) |

## Restore the release assets

```sh
gh release download spike/query-parser-v2 -R timohueser/OpenBikeComputer -D /tmp/qp
tar -xzf /tmp/qp/query-parser-v2-int8.tar.gz        # -> speed/work/onnx/ckpt-v2/
tar -xzf /tmp/qp/query-parser-v2-checkpoint.tar.gz  # -> work/ckpt-v2/
tar -xzf /tmp/qp/mmbert-small-cut50k.tar.gz         # -> speed/work/cut-50k/
tar -xzf /tmp/qp/query-parser-v2-data.tar.gz        # -> data/
```

Run the `tar` commands in this folder. `SHA256SUMS` in the release checks the files.

## Environments

Two `uv` environments, pinned:

```sh
uv venv .venv && uv pip install --python .venv/bin/python -r requirements.txt
cd speed && uv venv .venv && uv pip install --python .venv/bin/python -r requirements.txt
```

`.venv` runs the decoder, the generator and the rules. `speed/.venv` holds torch, transformers
and onnxruntime; it runs training, export, evaluation and the playground.

## Run

```sh
speed/.venv/bin/python playground/serve.py            # http://localhost:8770
export HF_HOME=speed/work/hf PYTHONHASHSEED=0
speed/.venv/bin/python evaluate.py onnx:speed/work/onnx/ckpt-v2/model.int8.onnx
.venv/bin/python evaluate.py rules
.venv/bin/python -m pytest -q test_decode.py
```

## Rebuild from scratch

```sh
cd speed && .venv/bin/python cut_vocab.py --top 50000 --elision --out work/cut-50k && cd ..
.venv/bin/python gen/fetch_names.py                   # Wikidata; changes over time, so prefer the asset
for l in en de; do .venv/bin/python gen/generate.py --lang $l --n 16000 --seed 7 --out data/train/$l.jsonl --filter; done
for l in fr it; do .venv/bin/python gen/generate.py --lang $l --n 10000 --seed 7 --out data/train/$l.jsonl --filter; done
speed/.venv/bin/python train.py --base speed/work/cut-50k \
  --train data/train/{en,de,fr,it}.jsonl --dev data/dev/{en,de,fr,it}.jsonl --out work/ckpt-v2 --epochs 3
cd speed && .venv/bin/python export_onnx.py --checkpoint ../work/ckpt-v2 --out work/onnx/ckpt-v2
```

- Training takes about 30 min on an M1 Pro GPU (MPS) and needs about 6 GB of free memory.
- The same seed and the same lexicon give byte-identical data. A lexicon change changes the filter
  pass, so generate again after each one.
- `cut_vocab.py` downloads the FrequencyWords lists and the base model from Hugging Face
  (`jhu-clsp/mmBERT-small`) into `speed/work/`.

## What will bite you

- **int8:** the MLP output layers of blocks 11 and 18 get inputs up to about 9,400. Plain dynamic
  int8 destroys the word labels. `export_onnx.py` keeps those two layers in fp32. Keep that step
  in any new export.
- **Word alignment:** labels live on `words.split` words; each word's first token carries its
  label. A new tokenizer must keep character offsets.
- **Test independence:** never let `testset/` shape the templates or the lexicon. v2 already had
  one fix round chosen from test errors.
- **Browser threads:** onnxruntime-web uses more than one thread only with COOP/COEP headers.
  Without them it runs on one thread (about 45 ms p95).
