"""Retain a checked native artifact for one known Local app."""

import json
import os
from pathlib import Path
import shutil
import subprocess

from . import step_request


def step(request):
    root = Path.cwd().resolve()
    code = request["options"]["code"]
    package = request["options"]["package"]
    if package not in ("planner-service", "obc-sim"):
        raise ValueError("Unknown Local native package")
    if str(root) != request["options"]["root"]:
        raise ValueError("Route service request names another checkout")
    environment = {**os.environ, "RUSTUP_AUTO_INSTALL": "0", "OBC_ROUTE_BUILD_ROOT": str(root),
                   "OBC_ROUTE_BUILD_CODE": code}
    for key in ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"):
        environment.pop(key, None)
    cargo = subprocess.run(["cargo", "build", "--locked", "--offline", "--release", "-p", package,
                            "--bin", package, "--message-format=json"],
                           env=environment, capture_output=True, text=True)
    if cargo.returncode:
        raise ValueError(cargo.stderr.strip())
    files = [value["executable"] for line in cargo.stdout.splitlines()
             if (value := json.loads(line)).get("reason") == "compiler-artifact"
             and value["target"]["name"] == package and value.get("executable")]
    if len(files) != 1:
        raise ValueError("Cargo returned no unique Local executable")
    output = Path(request["output"]) / package
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(files[0], output)
    output.chmod(0o500)
    identity = subprocess.run([str(output), "--obc-build-identity"], capture_output=True, text=True, check=True)
    if json.loads(identity.stdout) != {"root": str(root), "code": code}:
        raise ValueError("Compiled route service belongs to another root or execution identity")


if __name__ == "__main__":
    step(step_request.read())
