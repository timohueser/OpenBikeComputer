#!/usr/bin/env python3
"""Download the pinned NG1 selection and write native assembly sidecars."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
from urllib.request import Request, urlopen

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('output', type=Path)
args = parser.parse_args()
manifest = json.loads(Path(__file__).with_name('inputs.json').read_text())
args.output.mkdir(parents=True, exist_ok=True)

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def fetch(entry):
    suffix = '.obcd' if entry['band'] == 'terrain' else '.obcm'
    path = args.output / (entry['sha256'] + suffix)
    if not path.exists() or digest(path) != entry['sha256']:
        request = Request(entry['url'], headers={'User-Agent': 'OpenBikeComputer-NG1/1'})
        temporary = path.with_suffix('.part')
        with urlopen(request, timeout=120) as source, temporary.open('wb') as target:
            while block := source.read(1024 * 1024):
                target.write(block)
        if temporary.stat().st_size != entry['bytes'] or digest(temporary) != entry['sha256']:
            raise ValueError('Input size or digest mismatch: ' + entry['id'])
        temporary.replace(path)
    if path.stat().st_size != entry['bytes']:
        raise ValueError('Input size mismatch: ' + entry['id'])
    return dict(entry, path=str(path.resolve()))

with ThreadPoolExecutor(max_workers=4) as pool:
    entries = list(pool.map(fetch, manifest['objects']))
for name, data in {
    'cells.json': {'schema': manifest['schema'], 'cells': [e for e in entries if e['band'] != 'terrain']},
    'terrain.json': dict(manifest['terrain'], cells=[e for e in entries if e['band'] == 'terrain']),
    'skin.json': manifest['skin'],
}.items():
    (args.output / name).write_text(json.dumps(data, indent=2) + '\n')
print(f"Verified {len(entries)} objects, {sum(e['bytes'] for e in entries)} bytes")
