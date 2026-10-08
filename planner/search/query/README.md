# Query model development

Run these commands from this folder. The parent setup command installs the runtime.
Training needs a separate environment with PyTorch:

```sh
UV_PROJECT_ENVIRONMENT="$PWD/speed/.venv" uv sync --locked --project ../../.. --group search-training
```

Restore the pinned inputs from the repository's model release:

```sh
gh release download spike/query-parser-v2 -R timohueser/OpenBikeComputer -D ../data/training
(cd ../data/training && shasum -a 256 -c SHA256SUMS)
tar -xzf ../data/training/query-parser-v2-data.tar.gz
tar -xzf ../data/training/mmbert-small-cut50k.tar.gz
tar -xzf ../data/training/query-parser-v2-checkpoint.tar.gz
```

The archives restore `data/`, `speed/work/cut-50k/`, and `work/ckpt-v2/`.
They remain ignored by Git. Use `gen/fetch_names.py` only to refresh the Wikidata names.
Refreshes change the generated data.

To regenerate the training rows and train:

```sh
export HF_HOME=speed/work/hf PYTHONHASHSEED=0
for lang in en de; do ../.venv/bin/python gen/generate.py --lang "$lang" --n 16000 --seed 7 --out "data/train/$lang.jsonl" --filter; done
for lang in fr it; do ../.venv/bin/python gen/generate.py --lang "$lang" --n 10000 --seed 7 --out "data/train/$lang.jsonl" --filter; done
speed/.venv/bin/python train.py --base speed/work/cut-50k --train data/train/{en,de,fr,it}.jsonl --dev data/dev/{en,de,fr,it}.jsonl --out work/checkpoint --epochs 3
(cd speed && .venv/bin/python export_onnx.py --checkpoint ../work/checkpoint --out work/onnx/checkpoint)
```

`speed/cut_vocab.py` can rebuild the cut encoder from the base model and FrequencyWords
lists. See `--help`. The archived encoder is the stable input for reproduction.

The ONNX export keeps layers with large calibration activations in fp32. Plain dynamic
int8 quantization of these layers damages the word labels. Keep outlier detection enabled. Labels
refer to `words.split` words. Inference aligns each label with its first tokenizer token.

Training writes `training.json` with arguments, dependency versions, and SHA-256 hashes
of the encoder and data inputs. Keep this file with the checkpoint. A seed does not
guarantee identical weights across devices.

Export requires trained heads and a matching `labels.json`. It copies the label contract
and training record into the runtime directory. The runtime rejects a different label
order. Export parity checks do not replace held-out evaluation. Use
`--compression-sizes` only when you need gzip and Brotli measurements.

Evaluate the exported directory before use:

```sh
../.venv/bin/python evaluate.py runtime:speed/work/onnx/checkpoint
```

To try the export locally, keep the existing SQLite files in `data/`. Copy
`model.int8.onnx`, `tokenizer.json`, `labels.json`, and `training.json` from the export
into the parent `data/model/`, then restart the local server. Setup restores the pinned
model, so do not run setup after this replacement. Do not commit generated model files.

`schema.py` owns the finite model language. After a change, run `python3 schema.py` to
write `contract.json`; `test_artifacts.py` fails while the file is stale. `decode.py`
converts word tags to requests.
`runtime.py` runs CPU inference through ONNX Runtime. The local service also supports
UI-only map anchors and cuisine filters; the model does not emit these fields.
