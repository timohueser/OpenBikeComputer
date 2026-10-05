"""Build planner map and search inputs from one OSM snapshot."""

import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile

from . import data_registry, planner_maps as maps
from .planner_runtime import digest, open_url

# The source versions that this planner bake reads. `obc data` takes them from the live release.
VERSIONS = {
    "hansen-gfc": "v1.11",
    "protomaps-assets": "028c18f713baecad011301ff7a69acc39bcc2ae7",
    "tangrams-icons": "92510779634f4a006c61ea70e50cb8c52c765a81",
    "planetiler": "0.10.2",
    "protomaps-basemaps": "42ffaaa4a85a41bfcb23e43cc0f5b492a5eca123",
    "nominatim-country-data": "5.3.2",
}
ASSETS_URL = f"https://codeload.github.com/protomaps/basemaps-assets/zip/{VERSIONS['protomaps-assets']}"
PROTOMAPS = VERSIONS["protomaps-basemaps"]
PROTO_SHA = "f89ff8ee6aff13baf60c83b5e98d3811ddb946cc1089d437c435885395764696"
COUNTRY_DATA_VERSION = VERSIONS["nominatim-country-data"]
COUNTRY_DATA_URL = data_registry.SOURCES["nominatim-country-data"]["fetch"]["url"].format(version=COUNTRY_DATA_VERSION)
COUNTRY_DATA_SHA = "c5e1c4bd27f52a48843a5fe204a1a7b5f4d4d2911880e65721b4c900b13227e5"


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
        cached = download(item["url"], cache / "auxiliary" / name, item["sha256"])
        (directory / name).unlink(missing_ok=True)
        os.link(cached, directory / name)
    jar = source / "tiles/target/protomaps-basemap-HEAD-with-deps.jar"
    if not jar.exists():
        maps.run("mvn", "-q", "package", "-DskipTests", cwd=source / "tiles")
    maps.run("java", "-Xmx6g", "-jar", jar, "--download", f"--osm-path={osm}",
             f"--output={output}", "--bounds=" + ",".join(map(str, bounds)),
             "--maxzoom=14", f"--threads={os.cpu_count()}", cwd=source / "tiles")
    return {"protomaps_commit": PROTOMAPS, "planetiler": VERSIONS["planetiler"],
            "auxiliary": {p.name: {"sha256": digest(p), "bytes": p.stat().st_size}
                          for p in sorted(directory.iterdir()) if p.is_file()}}


def search_dump(osm, output, cache, country):
    archive = download(COUNTRY_DATA_URL, cache / 'nominatim-country-data.whl', COUNTRY_DATA_SHA)
    with tempfile.TemporaryDirectory(prefix='.country-data-', dir=cache) as directory:
        data = Path(directory)
        maps.run('uv', 'run', '--no-project', '--with', 'PyYAML==6.0.2', 'python',
                 maps.ROOT / 'host/obc-search-bake/policy.py', archive, data, cwd=maps.ROOT)
        maps.run('cargo', 'build', '--locked', '--release', '-p', 'obc-search-bake', '--bin', 'search-bake', cwd=maps.ROOT)
        maps.run(maps.ROOT / 'target/release/search-bake', osm, '--output', output,
                 '--default-country', country.lower(), '--policy', data / 'policy.json',
                 '--country-grid', data / 'country_osm_grid.sql.gz')
        return {'generator': 'obc-search-bake', 'country_data_sha256': COUNTRY_DATA_SHA,
                'policy_sha256': digest(data / 'policy.json'), 'country_grid_sha256': digest(data / 'country_osm_grid.sql.gz'),
                'dump_sha256': digest(output)}
