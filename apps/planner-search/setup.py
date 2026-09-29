"""Prepare local dependencies, the pinned model, and optional Germany search data."""
import argparse
import hashlib
import json
import sys
import shutil
import sqlite3
import signal
import subprocess
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent
MODEL_SHA = 'a8e1a42794fc101aace3982b991ce9f1f8852fcc5da1c056ddd1fe2699e8e48a'
DUMP_SHA = 'cfda04edd2de41f6aaae7469b9054eca13375b42ac2a448940b1bdcc6f052dac'
DUMP_URL = 'https://download1.graphhopper.com/public/europe/germany/photon-dump-germany-release-260920.jsonl.zst'


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def verify(path, expected):
    with path.open('rb') as stream:
        actual = hashlib.file_digest(stream, 'sha256').hexdigest()
    if actual != expected:
        raise SystemExit(f'Checksum mismatch: {path}. Remove this download and try again.')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--build-data', action='store_true', help='Download the Germany source and build the selected regional packages.')
    ap.add_argument('--data-dir', type=Path, default=ROOT / 'data')
    ap.add_argument('--region', choices=['germany', 'baden-wuerttemberg', 'all'], default='all')
    args = ap.parse_args()
    data = args.data_dir.resolve()
    data.mkdir(parents=True, exist_ok=True)
    run('npm', 'ci')
    if not (ROOT / '.venv').exists():
        run('uv', 'venv', '--python', '>=3.12', '.venv')
    run('uv', 'pip', 'install', '--python', '.venv/bin/python', '-r', 'requirements.txt', '-r', 'query/requirements.txt')
    run('npm', 'ci', '--prefix', '../../builder/app')
    archive = data / 'query-parser-v2-int8.tar.gz'
    if not archive.exists():
        run('gh', 'release', 'download', 'spike/query-parser-v2', '-R', 'timohueser/OpenBikeComputer',
            '-p', archive.name, '-D', str(data))
    verify(archive, MODEL_SHA)
    model = data / 'model'
    model.mkdir(exist_ok=True)
    with tarfile.open(archive) as tf:
        for name in ('model.int8.onnx', 'tokenizer.json', 'tokenizer_config.json'):
            member = tf.getmember(f'speed/work/onnx/ckpt-v2/{name}')
            if not member.isfile():
                raise SystemExit(f'Invalid model member: {name}')
            with tf.extractfile(member) as src, (model / name).open('wb') as dest:
                shutil.copyfileobj(src, dest)
    sys.path.insert(0, str(ROOT / 'query'))
    from artifacts import label_contract
    labels = json.dumps(label_contract())
    if hashlib.sha256(labels.encode()).hexdigest() != 'f0f50354f70c53a1868d8143abc271145d7d4bb968b7e690fb1a0fb9d428215a':
        raise SystemExit('The pinned model does not match the query schema. Train a new model.')
    (model / 'labels.json').write_text(labels)
    if args.build_data:
        source = data / 'germany.jsonl.zst'
        if not source.exists():
            partial = source.with_suffix('.download')
            print('Downloading the Germany source.', flush=True)
            urllib.request.urlretrieve(DUMP_URL, partial)
            partial.rename(source)
        verify(source, DUMP_SHA)
        regions = ['germany', 'baden-wuerttemberg'] if args.region == 'all' else [args.region]
        for region in regions:
            package = data / f'{region}.sqlite'
            if not package.exists():
                with tempfile.TemporaryDirectory(prefix='.search-', dir=data) as stage:
                    run('.venv/bin/python', 'build.py', str(source), '--output', stage, '--region', region)
                    Path(stage, package.name).rename(package)
            try:
                with sqlite3.connect(f'{package.as_uri()}?mode=ro', uri=True) as db:
                    schema = db.execute("SELECT value FROM metadata WHERE key='schema'").fetchone()
                    if schema != ('1',) or db.execute('PRAGMA quick_check').fetchone() != ('ok',):
                        raise ValueError('Incomplete data')
            except (sqlite3.Error, ValueError):
                raise SystemExit(f'Invalid search package: {package}. Move it aside, then repeat setup.')
    print(f'Search data ready: {data}')


if __name__ == '__main__':
    def stop(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, stop)
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
