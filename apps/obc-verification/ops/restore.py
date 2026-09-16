#!/usr/bin/env python3
"""Validate and restore a backup into a NEW directory; never overwrite live data."""
import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import tarfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    args.destination.mkdir(mode=0o700, parents=False, exist_ok=False)
    with tarfile.open(args.archive) as archive:
        archive.extractall(args.destination, filter="data")
    with sqlite3.connect(f"file:{args.destination / 'verification.sqlite'}?mode=ro", uri=True) as database:
        assert database.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        for row in database.execute("SELECT body FROM records WHERE kind='file'"):
            record = json.loads(row[0])
            content = (args.destination / "files" / record["id"]).read_bytes()
            assert len(content) == record["size"], f"Invalid size: {record['id']}"
            assert hashlib.sha256(content).hexdigest() == record["sha256"], f"Invalid hash: {record['id']}"
    print(f"Verified restored database and attachments: {args.destination}")


if __name__ == "__main__":
    main()
