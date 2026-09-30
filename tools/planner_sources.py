"""Build planner map and search inputs from one OSM snapshot."""

import getpass
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
from urllib.request import Request, urlopen

try:
    from . import planner_maps as maps
except ImportError:
    import planner_maps as maps

PROTOMAPS = "42ffaaa4a85a41bfcb23e43cc0f5b492a5eca123"
PROTO_SHA = "f89ff8ee6aff13baf60c83b5e98d3811ddb946cc1089d437c435885395764696"
PHOTON_URL = "https://github.com/komoot/photon/releases/download/1.3.0/photon-1.3.0.jar"
PHOTON_SHA = "a89707c0045e4807b2a1180e132e68e108d998709f48b6c94b98a6e281f571a5"


def open_url(url, timeout=60):
    request = Request(url) if isinstance(url, str) else url
    request.add_header("User-Agent", "OpenBikeComputer/1.0")
    return urlopen(request, timeout=timeout)


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def download(url, path, sha):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        partial = path.with_suffix(".download")
        with open_url(url, timeout=120) as source, partial.open("wb") as destination:
            shutil.copyfileobj(source, destination)
        if digest(partial) != sha:
            raise ValueError(f"Source checksum mismatch: {url}")
        partial.rename(path)
    if digest(path) != sha:
        raise ValueError(f"Cached source checksum mismatch: {path}")
    return path


def basemap(osm, output, bounds, cache, auxiliary=None):
    archive = download(f"https://codeload.github.com/protomaps/basemaps/tar.gz/{PROTOMAPS}",
                       cache / "protomaps.tar.gz", PROTO_SHA)
    source = cache / f"basemaps-{PROTOMAPS}"
    if not source.exists():
        with tarfile.open(archive) as bundle:
            bundle.extractall(cache, filter="data")
    directory = source / "tiles/data/sources"
    for name, item in (auxiliary or {}).items():
        if Path(name).name != name: raise ValueError("Invalid auxiliary source name")
        download(item["url"], directory / name, item["sha256"])
    jar = source / "tiles/target/protomaps-basemap-HEAD-with-deps.jar"
    if not jar.exists():
        maps.run("mvn", "-q", "package", "-DskipTests", cwd=source / "tiles")
    maps.run("java", "-Xmx2g", "-jar", jar, "--download", f"--osm-path={osm}",
             f"--output={output}", "--bounds=" + ",".join(map(str, bounds)),
             "--maxzoom=14", "--threads=2", cwd=source / "tiles")
    return {"protomaps_commit": PROTOMAPS, "planetiler": "0.10.2",
            "auxiliary": {p.name: {"sha256": digest(p), "bytes": p.stat().st_size}
                          for p in sorted(directory.iterdir()) if p.is_file()}}


def search_dump(osm, output, cache, port=5434):
    if os.geteuid() == 0:
        raise ValueError("Run source preparation as a normal user. PostgreSQL cannot run as root.")
    version = subprocess.check_output(["nominatim", "--version"], text=True)
    if "5.3.2" not in version:
        raise ValueError("Install nominatim-db==5.3.2 in the build environment.")
    photon = download(PHOTON_URL, cache / "photon.jar", PHOTON_SHA)
    pg = Path(subprocess.check_output(["pg_config", "--bindir"], text=True).strip())
    maps.check_port(port)
    with tempfile.TemporaryDirectory(prefix=".nominatim-", dir=cache) as directory:
        work = Path(directory)
        database, project, socket = work / "postgres", work / "project", work / "socket"
        project.mkdir()
        socket.mkdir()
        maps.run(pg / "initdb", "-D", database, "--auth-local=trust", "--auth-host=trust",
                 "--encoding=UTF8", "--locale=C.UTF-8")
        maps.run(pg / "pg_ctl", "-D", database, "-l", work / "postgres.log", "-o",
                 f"-p {port} -k {socket} -h 127.0.0.1 -c shared_buffers=256MB -c maintenance_work_mem=512MB", "start")
        try:
            maps.run(pg / "createuser", "-h", "127.0.0.1", "-p", port, "www-data")
            env = {**os.environ, "NOMINATIM_DATABASE_DSN":
                   f"pgsql:dbname=nominatim;host=127.0.0.1;port={port};user={getpass.getuser()}",
                   "NOMINATIM_IMPORT_STYLE": "extratags"}
            maps.run("nominatim", "import", "--project-dir", project, "--osm-file", osm,
                     "--reverse-only", "--no-updates", "--no-partitions", "--osm2pgsql-cache", "650", "-j", "2", env=env)
            maps.run(pg / "psql", "-h", "127.0.0.1", "-p", port, "-d", "nominatim", "-c",
                     "CREATE INDEX placex_country_code_idx ON placex(country_code);")
            partial = output.with_suffix(".download")
            export = subprocess.Popen(["java", "-Xmx1g", "-jar", str(photon), "dump-nominatim-db",
                                       "-host", "127.0.0.1", "-port", str(port), "-user", getpass.getuser(),
                                       "-extra-tags", "ALL", "-full-geometries", "-export-file", "-"], stdout=subprocess.PIPE, start_new_session=True)
            try:
                with export.stdout, partial.open("wb") as stream:
                    compressed = subprocess.run(["zstd", "-3", "-q"], stdin=export.stdout, stdout=stream)
                if export.wait() or compressed.returncode:
                    raise ValueError("Nominatim export failed")
            finally:
                if export.poll() is None: maps.stop_process(export)
            partial.rename(output)
        finally:
            maps.run(pg / "pg_ctl", "-D", database, "stop", "-m", "fast")
    return {"nominatim": "5.3.2", "photon": "1.3.0", "photon_sha256": PHOTON_SHA,
            "dump_sha256": digest(output), "importance": "Nominatim default; no external Wikipedia ranks"}
