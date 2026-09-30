"""Stage the notices for the pinned native parser and its bundled dependencies."""
import hashlib
import importlib.metadata
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile
import urllib.request

LICENSE = re.compile(r'^(licen[cs]e|unlicense|copying|copyright|notice)([._-].*)?$', re.I)


def stage_notices(destination):
    notices = []

    def add(name, text):
        notices.append(f'{name}\n{"=" * 72}\n{text.strip()}\n')

    repository = Path(__file__).resolve().parents[4]
    for name in ['LICENSE', 'THIRD-PARTY.md']:
        add(f'OpenBikeComputer {name}', (repository / name).read_text())
    for name, path in [('CPython 3.14.6', 'python/Python.xcframework/lib/python3.14/LICENSE.txt'),
                       ('ONNX Runtime 1.30.0', 'ort/LICENSE')]:
        add(name, (destination / path).read_text())
    cache = destination / 'notices'
    cache.mkdir(exist_ok=True)
    for name, entry in json.loads(Path(__file__).with_name('parser-notices.json').read_text()).items():
        path = cache / entry['sha256']
        if not path.exists():
            with urllib.request.urlopen(entry['url']) as response:
                path.write_bytes(response.read())
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != entry['sha256']:
            raise ValueError(f'{name}: notice source hash differs')
        if 'member' in entry:
            with tarfile.open(fileobj=io.BytesIO(data)) as archive:
                data = archive.extractfile(entry['member']).read()
        add(f'{name}\n{entry["url"]}', data.decode())
    for name in ['rapidfuzz', 'PyYAML', 'snowballstemmer']:
        distribution = importlib.metadata.distribution(name)
        files = [file for file in distribution.files if LICENSE.match(file.name)]
        if not files:
            raise ValueError(f'{name}: distribution has no license text')
        for file in sorted(files):
            add(f'{name} {distribution.version}: {file.name}', distribution.locate_file(file).read_text())
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--filter-platform', 'aarch64-apple-ios', '--format-version=1',
        '--manifest-path', str(destination / 'tokenizer/Cargo.toml')]))
    resolved = {node['id'] for node in metadata['resolve']['nodes']}
    for package in sorted(metadata['packages'], key=lambda package: (package['name'], package['version'])):
        if package['source'] is None or package['id'] not in resolved:
            continue
        root = Path(package['manifest_path']).parent
        files = sorted(file for file in root.rglob('*') if file.is_file() and LICENSE.match(file.name))
        if not files:
            raise ValueError(f'{package["name"]}: crate has no license text')
        for file in files:
            add(f'{package["name"]} {package["version"]}: {file.relative_to(root)}', file.read_text())
    output = destination / 'app/native-third-party-licenses.txt'
    output.write_text('\n'.join(notices))
    return output
