"""Copy pinned browser assets and the archived query model into the playground."""
import ast
import json
import shutil
import subprocess
from pathlib import Path

root = Path(__file__).parent
vendor = root / 'web/vendor'
vendor.mkdir(exist_ok=True)
shutil.copy2(root.parent.parent / "docs/assets/fonts/atkinson-hyperlegible-next-latin.woff2", vendor / "atkinson.woff2")
for package, source, target in [
    ('@sqlite.org/sqlite-wasm', 'dist', 'sqlite'),
    ('leaflet', 'dist', 'leaflet'),
    ('onnxruntime-web', 'dist', 'ort'),
    ('@huggingface/tokenizers', 'dist', 'tokenizers'),
]:
    dest = vendor / target
    dest.mkdir(exist_ok=True)
    src = root / 'node_modules' / package / source
    for item in src.iterdir():
        if item.is_file() and (item.suffix in ('.mjs', '.wasm', '.css', '.js') or item.name == 'LICENSE'):
            shutil.copy2(item, dest / item.name)
    license_path = root / 'node_modules' / package / 'LICENSE'
    if license_path.exists():
        shutil.copy2(license_path, dest / 'LICENSE')

model = root / 'parser/speed/work/onnx/ckpt-v2'
dest = vendor / 'model'
dest.mkdir(exist_ok=True)
for filename in ('model.int8.onnx', 'tokenizer.json', 'tokenizer_config.json'):
    shutil.copy2(model / filename, dest / filename)
source = subprocess.check_output(['git', 'show', 'spike/query-parser-v2:spike/query-parser/schema.py'], text=True)
labels = {}
for n in ast.parse(source).body:
    if isinstance(n, ast.Assign) and isinstance(n.targets[0], ast.Name) and n.targets[0].id in ('INTENTS', 'SLOTS'):
        labels[n.targets[0].id] = ast.literal_eval(n.value)
labels['LABELS'] = ['O'] + [f'{p}-{s}' for s in labels['SLOTS'] for p in ('B','I')]
(root / 'web/labels.json').write_text(json.dumps(labels))
lexicon = subprocess.check_output(['git','show','spike/query-parser-v2:spike/query-parser/lexicon/kinds.json'])
(root / 'web/kinds.json').write_bytes(lexicon)
print('Browser assets are ready.')
