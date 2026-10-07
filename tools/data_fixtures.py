"""Import exact fixture archives and seal packages through the shared fixture format."""

import argparse
import json
from pathlib import Path

from . import fixtures


def imported(package, archive, digest, destination):
    if fixtures.sha256_file(archive) != digest:
        raise fixtures.FixtureError(f"{package}: archive differs from its pinned SHA-256")
    if destination.exists():
        raise fixtures.FixtureError(f"{package}: import destination must be absent")
    destination.mkdir(parents=True)
    fixtures.extract_package_archive(archive, destination, package)
    manifest = json.loads((destination / fixtures.MANIFEST_NAME).read_bytes())
    return {"package": package, "archive": digest, "files": manifest["files"]}


def sealed(package, source, archive):
    size, digest = fixtures.build_package(package, source, archive)
    return {"package": package, "bytes": size, "sha256": digest}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    capture = commands.add_parser("import")
    capture.add_argument("package")
    capture.add_argument("archive", type=Path)
    capture.add_argument("sha256")
    capture.add_argument("destination", type=Path)
    seal = commands.add_parser("seal")
    seal.add_argument("package")
    seal.add_argument("source", type=Path)
    seal.add_argument("archive", type=Path)
    args = parser.parse_args()
    if args.command == "import":
        result = imported(args.package, args.archive, args.sha256, args.destination)
    else:
        result = sealed(args.package, args.source, args.archive)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
