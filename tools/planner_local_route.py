"""Build the current host's route service and retain its checked artifact."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from . import step_request


def step(request):
    root = Path.cwd().resolve()
    code = request["options"]["code"]
    environment = {**os.environ, "RUSTUP_AUTO_INSTALL": "0", "OBC_ROUTE_BUILD_ROOT": str(root),
                   "OBC_ROUTE_BUILD_CODE": code}
    for key in ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"):
        environment.pop(key, None)
    cargo = subprocess.run(["cargo", "build", "--locked", "--offline", "--release", "-p", "route-server",
                            "--bin", "route-server", "--message-format=json"],
                           env=environment, capture_output=True, text=True)
    if cargo.returncode:
        raise ValueError(cargo.stderr.strip())
    files = [value["executable"] for line in cargo.stdout.splitlines()
             if (value := json.loads(line)).get("reason") == "compiler-artifact"
             and value["target"]["name"] == "route-server" and value.get("executable")]
    if len(files) != 1:
        raise ValueError("Cargo returned no unique route-server executable")
    output = Path(request["output"]) / "route-server"
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(files[0], output)
    output.chmod(0o500)
    identity = subprocess.run([str(output), "--obc-build-identity"], capture_output=True, text=True, check=True)
    if json.loads(identity.stdout) != {"root": str(root), "code": code}:
        raise ValueError("Compiled route service belongs to another root or execution identity")


if __name__ == "__main__":
    step(step_request.read())
