#!/usr/bin/env python3
"""Measure three native assemblies with the pinned options and full verification."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('inputs', type=Path)
parser.add_argument('output', type=Path)
parser.add_argument('--check-inputs', action='store_true', help='Verify pinned local inputs without running assembly')
args = parser.parse_args()
manifest = json.loads(Path(__file__).with_name('inputs.json').read_text())
# Check the exact local inputs before any measured assembly or output creation.
if set(manifest['options']) != {'merge_budget_bytes', 'accept_partial', 'accept_holes', 'full_verification'} or manifest['options']['full_verification'] is not True:
    raise ValueError('Unsupported options or disabled full verification in manifest')
if json.loads((args.inputs / 'skin.json').read_text()) != manifest['skin']:
    raise ValueError('Skin differs from pinned manifest')
for name, terrain in [('cells.json', False), ('terrain.json', True)]:
    sidecar = json.loads((args.inputs / name).read_text())
    entries = sidecar.pop('cells')
    header = manifest['terrain'] if terrain else {'schema': manifest['schema']}
    if sidecar != header:
        raise ValueError(f'{name}: schema or terrain lattice differs from pinned manifest')
    expected = [entry for entry in manifest['objects'] if (entry['band'] == 'terrain') == terrain]
    if len(entries) != len(expected):
        raise ValueError(f'{name}: cell count differs from pinned manifest')
    for entry, pinned in zip(entries, expected):
        if {key: value for key, value in entry.items() if key != 'path'} != pinned:
            raise ValueError(f'{name}: cell identity, order or metadata differs from pinned manifest')
        path = args.inputs / entry['path']
        if path.stat().st_size != pinned['bytes']:
            raise ValueError(f'{path}: input size differs from pinned manifest')
        with path.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        if digest != pinned['sha256']:
            raise ValueError(f'{path}: input digest differs from pinned manifest')
if args.check_inputs:
    print(f"Verified {len(manifest['objects'])} pinned local inputs; no assembly ran")
    raise SystemExit(0)
args.output.mkdir(parents=True, exist_ok=True)
reports = []
for repeat in range(3):
    output = args.output / f'native-{repeat}.obcm'
    command = [str(args.binary.resolve()), '--cells', str(args.inputs / 'cells.json'), '--skin', str(args.inputs / 'skin.json'), '--terrain', str(args.inputs / 'terrain.json'), '--out', str(output), '--merge-budget-bytes', str(manifest['options']['merge_budget_bytes']), '--json']
    if manifest['options']['accept_partial']:
        command.append('--accept-partial')
    if manifest['options']['accept_holes']:
        command.append('--accept-holes')
    result = subprocess.run(command, check=True, capture_output=True, text=True)
    summary = json.loads(result.stdout)
    with output.open('rb') as stream:
        readback = hashlib.file_digest(stream, 'sha256').hexdigest()
    assert readback == summary['sha256']
    assert output.stat().st_size == summary['bytes']
    assert 'nav-profile collect' in result.stderr, 'Build with --features mem-profile'
    (args.output / f'native-{repeat}.log').write_text(result.stderr)
    reports.append({'command': command, 'summary': summary, 'readback_sha256': readback, 'ledger': result.stderr})
assert len({r['readback_sha256'] for r in reports}) == 1, 'Output changes between repeats'
report = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), 'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(), 'environment_rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(), 'platform': platform.platform(), 'manifest_sha256': hashlib.sha256(Path(__file__).with_name('inputs.json').read_bytes()).hexdigest(), 'runs': reports}
(args.output / 'native.json').write_text(json.dumps(report, indent=2) + '\n')
print(args.output / 'native.json')
