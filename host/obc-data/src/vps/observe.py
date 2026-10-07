"""Select the installed downloads helper without fetching or preparing any files."""

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tarfile

request = json.load(sys.stdin)
unit = next(item for item in request['installed'] if item['service'] == 'downloads')
candidate = next(item for item in request['candidates'] if item['service'] == 'downloads')
base = Path('/opt/obc-planner/services')
expected = base / 'downloads' / unit['id']
result = subprocess.run(['systemctl', 'show', f"obc-planner-downloads-{unit['slot']}.service",
                         '--property=WorkingDirectory', '--value'], capture_output=True, text=True, check=True)
code = Path(result.stdout.strip())
if code != expected / 'code' or code.resolve() != code:
    raise ValueError('No owned installed downloads helper; apply the reviewed runtime first')
for name, item in [('release.json', candidate['release']), ('runtime.json', candidate['runtime'])]:
    path = expected / name
    if path.stat().st_size != item['size'] or hashlib.file_digest(path.open('rb'), 'sha256').hexdigest() != item['sha256']:
        raise ValueError('Installed helper metadata differs from publication')
descriptor = json.loads((expected / 'runtime.json').read_bytes())
archive = expected / 'runtime.tar.gz'
if archive.stat().st_size != descriptor['payload']['bytes'] or hashlib.file_digest(archive.open('rb'), 'sha256').hexdigest() != descriptor['payload']['sha256']:
    raise ValueError('Installed helper archive differs')
# Check import bytes before loading the artifact's existing full verifier.
with tarfile.open(archive, 'r:gz') as bundle:
    actual = {path.relative_to(code).as_posix() for path in code.rglob('*') if path.is_file() or path.is_symlink()}
    if actual != set(bundle.getnames()):
        raise ValueError('Installed helper file set differs')
    imports = ['planner_install.py', 'planner_offline.py', 'planner_runtime.py']
    if (code / 'tools/__init__.py').exists(): imports.append('__init__.py')
    for name in imports:
        name = 'tools/' + name
        entry = bundle.getmember(name)
        path = code / name
        if not entry.isfile() or path.is_symlink() or not path.resolve().is_relative_to(code) or hashlib.file_digest(path.open('rb'), 'sha256').hexdigest() != hashlib.file_digest(bundle.extractfile(entry), 'sha256').hexdigest():
            raise ValueError('Installed helper import differs')
sys.path.insert(0, str(code))
from tools import planner_install
planner_install.verify_code(archive, code)
print(json.dumps(planner_install.observe(request)))
