"""Prepare pinned native parser dependencies and the shared decoder for the phone harness."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]
DESTINATION = REPO / 'target/planner-parser'
ARCHIVES = {
    'python': ('https://github.com/beeware/Python-Apple-support/releases/download/3.14-b10/Python-3.14-iOS-support.b10.tar.gz',
               '200ef60eb67be0483ceb638daa9048f84f41a9a952707a5ad4c3198037c7b583', 'tar.gz'),
    'ort': ('https://download.onnxruntime.ai/pod-archive-onnxruntime-c-1.30.0.zip',
            'e6f1670c14406fd9f082bb400ab197a9b0a9646058ca6366e440642e2b54a2ea', 'zip'),
}


def archive(name, url, digest, extension):
    path = DESTINATION / f'{name}.{extension}'
    if not path.exists():
        temporary = path.with_suffix('.partial')
        urllib.request.urlretrieve(url, temporary)
        temporary.replace(path)
    with path.open('rb') as stream:
        if hashlib.file_digest(stream, 'sha256').hexdigest() != digest:
            raise ValueError(f'{path}: archive hash differs')
    directory = DESTINATION / name
    if not directory.exists():
        directory.mkdir()
        if extension == 'zip':
            # ditto preserves the macOS framework symlinks in the upstream archive.
            subprocess.run(['ditto', '-xk', str(path), str(directory)], check=True)
        else:
            with tarfile.open(path) as bundle:
                bundle.extractall(directory, filter='tar')
    return directory


def prepare(python, model):
    DESTINATION.mkdir(parents=True, exist_ok=True)
    for name, values in ARCHIVES.items():
        archive(name, *values)
    native = DESTINATION / 'tokenizer'
    native.mkdir(exist_ok=True)
    (native / 'Cargo.toml').write_text('''[package]
name = "obc-tokenizer-portability"
version = "0.0.0"
edition = "2021"
[workspace]
[lib]
path = "tokenizer.rs"
crate-type = ["staticlib"]
[dependencies]
tokenizers = { version = "=0.23.2", default-features = false, features = ["onig"] }
serde_json = "1"
''')
    shutil.copy2(HERE / 'tokenizer.rs', native / 'tokenizer.rs')
    shutil.copy2(HERE / 'tokenizer.lock', native / 'Cargo.lock')
    subprocess.run(['cargo', 'build', '--locked', '--release', '--manifest-path', str(native / 'Cargo.toml'),
                    '--target', 'aarch64-apple-ios'], check=True,
                   env={**os.environ, 'IPHONEOS_DEPLOYMENT_TARGET': '17.0'})
    subprocess.run([str(python), str(HERE / 'prepare-parser.py'), '--stage-python', str(model)], check=True)


def stage(model):
    import importlib.metadata
    import sys
    sys.path.insert(0, str(REPO / 'apps/planner-search/query'))
    from evaluate import load_testset
    from runtime import Parser
    import rapidfuzz
    import yaml
    import snowballstemmer
    for name, version in [('rapidfuzz', '3.14.6'), ('PyYAML', '6.0.3'), ('snowballstemmer', '3.1.1')]:
        if importlib.metadata.version(name) != version:
            raise ValueError(f'Stage {name}=={version}')
    app = DESTINATION / 'app'
    app.mkdir(exist_ok=True)
    source = REPO / 'apps/planner-search/query'
    for name in ['decode.py', 'lexicon.py', 'schema.py', 'words.py', 'prediction.py', 'artifacts.py']:
        shutil.copy2(source / name, app / name)
    for path in [REPO / 'tools/planner_runtime.py', REPO / 'tools/planner_offline.py', HERE / 'phone_install_benchmark.py']:
        shutil.copy2(path, app / path.name)
    shutil.copytree(source / 'lexicon', app / 'lexicon', dirs_exist_ok=True,
                    ignore=shutil.ignore_patterns('src', '__pycache__'))
    for module in [rapidfuzz, yaml, snowballstemmer]:
        shutil.copytree(Path(module.__file__).parent, app / module.__name__, dirs_exist_ok=True,
                        ignore=shutil.ignore_patterns('*.so', '*.pyc', '__pycache__', '*.pyi', '*.h', '*.pxd'))
    from parser_notices import stage_notices
    stage_notices(DESTINATION)
    parser = Parser(model)
    rows = [{'text': row['text'], 'request': parser.parse(row['text'])['request']} for row in load_testset(None)]
    hashes = {}
    for filename in ['model.int8.onnx', 'tokenizer.json', 'labels.json', 'tokenizer_config.json']:
        with (model / filename).open('rb') as stream:
            hashes[filename] = hashlib.file_digest(stream, 'sha256').hexdigest()
    (DESTINATION / 'parser-reference.json').write_text(json.dumps({'cases': rows, 'model': hashes}, ensure_ascii=False))
    print(json.dumps({'parser_cases': len(rows), 'output': str(DESTINATION)}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--python', type=Path, help='Search virtual environment interpreter')
    parser.add_argument('--model', type=Path)
    parser.add_argument('--stage-python', type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.stage_python:
        stage(args.stage_python)
    elif args.python and args.model:
        prepare(args.python, args.model)
    else:
        parser.error('Supply --python and --model')
