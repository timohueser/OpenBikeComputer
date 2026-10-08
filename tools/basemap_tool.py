"""Prepare the Protomaps and Planetiler executable from a pinned source archive."""

import argparse
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import xml.etree.ElementTree as ET


def prepare(archive, output):
    """Only tool preparation runs Maven, which can download packages."""
    with tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        root = Path(temporary)
        with tarfile.open(archive) as bundle:
            bundle.extractall(root, filter="data")
        source, = root.iterdir()
        tiles = source / "tiles"
        pom = ET.parse(tiles / "pom.xml")
        planetiler = pom.findtext("{*}properties/{*}planetiler.version")
        if not planetiler or "SNAPSHOT" in planetiler or not re.fullmatch(r"\d+\.\d+\.\d+(?:[.-][A-Za-z0-9]+)*", planetiler):
            raise ValueError("The basemap tool must pin a Planetiler release")
        subprocess.run(["mvn", "-B", "-q", "package", "-DskipTests",
                        "-Dproject.build.outputTimestamp=2024-01-01T00:00:00Z"],
                       cwd=tiles, check=True)
        shutil.copyfile(tiles / "target/protomaps-basemap-HEAD-with-deps.jar", output)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    prepare(args.archive, args.output)


if __name__ == "__main__":
    main()
