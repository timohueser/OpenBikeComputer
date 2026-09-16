#!/usr/bin/env python3
"""Snapshot SQLite and immutable attachments together. Run as root on the VPS."""
import argparse
import json
import re
from pathlib import Path
import shutil
import sqlite3
import tarfile
import tempfile
from datetime import datetime, timezone


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, default=Path("/var/lib/obc-verification"))
    parser.add_argument("--output", type=Path, default=Path("/var/backups/obc-verification"))
    parser.add_argument("--keep", type=int, default=7)
    args = parser.parse_args()
    if args.keep < 1:
        parser.error("--keep must be at least one")
    args.output.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.output) as temporary:
        snapshot = Path(temporary)
        with sqlite3.connect(f"file:{args.data / 'verification.sqlite'}?mode=ro", uri=True) as source:
            with sqlite3.connect(snapshot / "verification.sqlite") as target:
                source.backup(target)
                assert target.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
                files = [json.loads(row[0]) for row in target.execute("SELECT body FROM records WHERE kind='file'")]
        (snapshot / "files").mkdir()
        for item in files:
            # The database snapshot fixes the set; uploads after it are not needed.
            shutil.copyfile(args.data / "files" / item["id"], snapshot / "files" / item["id"])
        stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
        output = args.output / f"verification-{stamp}.tar.gz"
        with tarfile.open(output, "w:gz") as archive:
            archive.add(snapshot / "verification.sqlite", arcname="verification.sqlite")
            archive.add(snapshot / "files", arcname="files")
        output.chmod(0o600)
        print(output)
        snapshots = sorted(path for path in args.output.iterdir() if path.is_file() and re.fullmatch(r"verification-[0-9]{8}T[0-9]{12}Z\.tar\.gz", path.name))
        for old in snapshots[:-args.keep]:
            old.unlink()


if __name__ == "__main__":
    main()
