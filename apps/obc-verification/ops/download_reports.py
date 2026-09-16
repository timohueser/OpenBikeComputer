"""Download native reports for one GitHub run attempt; never reuse older attempts."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys

root = Path(sys.argv[1])
root.mkdir(parents=True, exist_ok=True)
run_id = os.environ["RUN_ID"]
attempt = os.environ["RUN_ATTEMPT"]
repo = os.environ["GH_REPO"]
pages = json.loads(subprocess.check_output([
    "gh", "api", f"repos/{repo}/actions/runs/{run_id}/artifacts?per_page=100", "--paginate", "--slurp"
]))
pattern = re.compile(r"(?:rust-(?:test|fixtures)|python-.+|web-.+|desktop-tests-.+|ios-tests-coverage)-" + re.escape(attempt))
for artifact in (artifact for page in pages for artifact in page["artifacts"]):
    name = artifact["name"]
    if not pattern.fullmatch(name):
        continue
    if artifact["expired"]:
        raise RuntimeError(f"Required report expired: {name}")
    subprocess.run(["gh", "run", "download", run_id, "--name", name, "--dir", str(root / name)], check=True)
