"""Prepare local dependencies and the pinned query model."""
import argparse
import hashlib
import json
import sys
import shutil
import signal
import subprocess
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
MODEL_SHA = 'a8e1a42794fc101aace3982b991ce9f1f8852fcc5da1c056ddd1fe2699e8e48a'


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def npm_ci(directory):
    """`npm ci` replaces node_modules and breaks a running Vite server, so it runs only for a changed lockfile."""
    lock = hashlib.sha256((directory / 'package-lock.json').read_bytes()).hexdigest()
    stamp = directory / 'node_modules/.obc-package-lock.sha256'
    if stamp.is_file() and stamp.read_text() == lock:
        return
    run('npm', 'ci', '--prefix', str(directory))
    stamp.write_text(lock)


def verify(path, expected):
    with path.open('rb') as stream:
        actual = hashlib.file_digest(stream, 'sha256').hexdigest()
    if actual != expected:
        raise SystemExit(f'{path}: SHA-256 {actual}, not the pinned {expected}')


def install_model(archive, data):
    """The model files of the query model archive in `data/model`, with the labels of the query schema."""
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


def step():
    """The `obc data` step `planner/model`: the model of the `query-model` snapshot."""
    sys.path.insert(0, str(ROOT.parents[1]))
    from tools import step_request

    request = step_request.read()
    (archive,) = step_request.files(request, 'query-model').values()
    verify(archive, MODEL_SHA)
    install_model(archive, Path(request['output']))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--data-dir', type=Path, default=ROOT / 'data')
    args = ap.parse_args()
    data = args.data_dir.resolve()
    data.mkdir(parents=True, exist_ok=True)
    npm_ci(ROOT)
    if not (ROOT / '.venv').exists():
        run('uv', 'venv', '--python', '>=3.12', '.venv')
    run('uv', 'pip', 'install', '--python', '.venv/bin/python', '-r', 'requirements.txt', '-r', 'query/requirements.txt')
    npm_ci(ROOT.parents[1] / 'builder/app')
    archive = data / 'query-parser-v2-int8.tar.gz'
    if not archive.exists():
        run('gh', 'release', 'download', 'spike/query-parser-v2', '-R', 'timohueser/OpenBikeComputer',
            '-p', archive.name, '-D', str(data))
    verify(archive, MODEL_SHA)
    install_model(archive, data)
    print(f'Search dependencies and model ready: {data}')


if __name__ == '__main__':
    def stop(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, stop)
    try:
        step() if sys.argv[1:] == ['--step'] else main()
    except KeyboardInterrupt:
        sys.exit(130)
