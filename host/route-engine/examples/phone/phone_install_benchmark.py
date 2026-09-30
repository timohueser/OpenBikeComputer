"""Measure the shared durable installer with a local complete planner bundle."""

import json
from pathlib import Path
import tempfile
import time

import planner_offline as offline


def run(root):
    root = Path(root)
    source = root / "offline-bundle"
    destination = Path(tempfile.mkdtemp(prefix="offline-installed-", dir=root))
    report = {"file_cache": "Uncontrolled OS cache; source bundle is verified before installation",
              "transport": "Local bundle reads; network latency is excluded",
              "destination": str(destination)}
    report["bundle"] = offline.verify_bundle(source)
    manifest = json.loads((source / "bundle.json").read_bytes())
    transport = max((entry["transport"] for entry in manifest["files"].values()), key=lambda entry: entry["bytes"])
    seeded = min(1 << 20, transport["bytes"] // 2)
    downloads = destination / "downloads"
    downloads.mkdir()
    with (source / "objects" / transport["sha256"]).open("rb") as stream:
        (downloads / transport["sha256"]).write_bytes(stream.read(seeded))
    report["resume_prefix_bytes"] = seeded
    report["resume_setup"] = "A verified source prefix simulates a prior interrupted transfer"
    for name in ("install", "repeat_install"):
        started = time.monotonic()
        result = offline.install(source, destination)
        result["elapsed_seconds"] = time.monotonic() - started
        active = json.loads((destination / "active.json").read_bytes())
        if active["release"] != result["release"]:
            raise ValueError("Active release differs from verified installed release")
        report[name] = result
    if seeded and report["install"]["downloaded_bytes"] != report["bundle"]["transfer_bytes"] - seeded:
        raise ValueError("Installer did not resume at the retained prefix")
    if report["repeat_install"]["added_stored_bytes"] != 0:
        raise ValueError("Repeated installation added retained objects")
    report["active_release_verified"] = True
    return report
