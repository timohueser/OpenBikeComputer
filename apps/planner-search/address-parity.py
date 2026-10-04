"""Build address packages from verified same-snapshot dumps and compare lookup output."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys

import zstandard


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('candidate', type=Path)
    ap.add_argument('reference', type=Path)
    ap.add_argument('--reference-inputs', type=Path, required=True)
    ap.add_argument('--output', type=Path, required=True)
    ap.add_argument('--countries', required=True)
    ap.add_argument('--sample-size', type=int, default=500)
    ap.add_argument('--require-equivalent', action='store_true')
    args = ap.parse_args()
    manifest = json.loads(args.reference_inputs.read_text())
    if digest(args.reference) != manifest['files']['search.jsonl.zst']:
        ap.error('Reference dump does not match its input manifest')
    with args.candidate.open('rb') as raw, zstandard.ZstdDecompressor().stream_reader(raw) as stream:
        header = json.loads(io.BufferedReader(stream).readline())['content']
    if header.get('osm_sha256') != manifest['osm_sha256']:
        ap.error('Candidate and reference use different OSM snapshots')
    if args.output.exists():
        ap.error('Choose a fresh comparison directory')
    root = Path(__file__).parent
    for name, source in [('candidate', args.candidate), ('reference', args.reference)]:
        subprocess.run([sys.executable, str(root / 'build.py'), str(source), '--component', 'addresses',
                        '--output', str(args.output / name), '--region', 'comparison',
                        '--bounds=' + ','.join(map(str, manifest['bounds'])),
                        '--countries=' + args.countries, '--osm-sha256=' + manifest['osm_sha256']], check=True)
    subprocess.run(['node', str(root / 'address-parity.mjs'),
                    str(args.output / 'candidate/comparison.sqlite'),
                    str(args.output / 'reference/comparison.sqlite'), str(args.sample_size),
                    *(['--require-equivalent'] if args.require_equivalent else [])], check=True)


if __name__ == '__main__':
    main()
