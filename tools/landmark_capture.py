#!/usr/bin/env python3
"""Capture geographic Wiki sources for the offline obc-bake landmark compiler."""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import subprocess
import tempfile
import threading
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode, unquote, urlparse
from urllib.request import Request, urlopen

LANGUAGE_BYTES = (Path(__file__).resolve().parents[1] / "specs/content-languages.json").read_bytes()
LANGUAGES = tuple(code for code, _ in json.loads(LANGUAGE_BYTES))
MAX_LOCALE_DEPTH = 8
MAX_LOCALES = 64
USER_AGENT = "OpenBikeComputer-landmark-capture/1.0 (https://github.com/timohueser/OpenBikeComputer)"
MAX_SOURCE = 32 * 1024 * 1024
MAX_CLASSES = 16384


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(path)


def api(site: str, **params: object) -> str:
    return f"https://{site}/w/api.php?" + urlencode({"format": "json", **params})


class Capture:
    """Each request is an immutable outcome; restart verifies and reuses its bytes."""

    def __init__(self, root: Path, interval: float = 0.25):
        self.root = root
        self.interval = interval
        self.lock = threading.Lock()
        self.path_locks = {}
        self.next_request = 0.0
        root.mkdir(parents=True, exist_ok=True)

    def fetch(self, path: str, url: str) -> dict:
        with self.lock:
            path_lock = self.path_locks.setdefault(path, threading.Lock())
        with path_lock:
            return self._fetch(path, url)

    def _fetch(self, path: str, url: str) -> dict:
        if Path(path).is_absolute() or ".." in Path(path).parts:
            raise ValueError("capture path must stay inside its directory")
        key = digest(path.encode())
        record = self.root / "outcomes" / (key + ".json")
        target = self.root / path
        if record.exists():
            outcome = json.loads(record.read_text())
            if outcome["url"] != url or outcome["path"] != path:
                raise ValueError(f"request changed for {path}; use a new capture directory")
            if "sha256" in outcome:
                data = target.read_bytes()
                if len(data) != outcome["bytes"] or digest(data) != outcome["sha256"]:
                    raise ValueError(f"captured bytes changed: {path}")
            return outcome
        with self.lock:
            delay = max(0, self.next_request - time.monotonic())
            self.next_request = max(time.monotonic(), self.next_request) + self.interval
        if delay:
            time.sleep(delay)
        outcome = {"path": path, "url": url, "retrieved_at": datetime.now(timezone.utc).isoformat()}
        try:
            with urlopen(Request(url, headers={"User-Agent": USER_AGENT}), timeout=75) as response:
                outcome["response_url"] = response.url
                outcome["http_status"] = response.status
                outcome["headers"] = {k: response.headers[k] for k in ("ETag", "Last-Modified", "Content-Type") if k in response.headers}
                if int(response.headers.get("Content-Length", 0)) > MAX_SOURCE:
                    raise ValueError("source exceeds 32 MiB acquisition bound")
                data = response.read(MAX_SOURCE + 1)
                if len(data) > MAX_SOURCE:
                    raise ValueError("source exceeds 32 MiB acquisition bound")
            target.parent.mkdir(parents=True, exist_ok=True)
            temporary = target.with_suffix(target.suffix + ".tmp")
            temporary.write_bytes(data)
            temporary.replace(target)
            outcome.update(status="ok", bytes=len(data), sha256=digest(data))
            if path.endswith(".json"):
                try:
                    value = json.loads(data)
                    valid = isinstance(value, dict) and "error" not in value
                except (UnicodeDecodeError, ValueError):
                    valid = False
                if not valid:
                    outcome.update(status="invalid-response", reason="response is not valid API JSON or contains an API error")
        except HTTPError as error:
            outcome.update(status="http-error", http_status=error.code, reason=str(error))
        except (URLError, TimeoutError, OSError) as error:
            outcome.update(status="transport-error", reason=str(error))
        except ValueError as error:
            outcome.update(status="oversized", reason=str(error))
        write_json(record, outcome)
        return outcome

    def json(self, path: str, url: str) -> dict | None:
        outcome = self.fetch(path, url)
        if outcome["status"] != "ok":
            return None
        return json.loads((self.root / path).read_bytes())

    def outcomes(self) -> list[dict]:
        return sorted((json.loads(p.read_text()) for p in (self.root / "outcomes").glob("*.json")), key=lambda v: v["path"])

    def retry_failed(self) -> None:
        for record in (self.root / "outcomes").glob("*.json"):
            outcome = json.loads(record.read_text())
            if outcome["status"] == "ok":
                continue
            history = self.root / "attempts" / (record.stem + "-" + digest(record.read_bytes()) + ".json")
            if "sha256" in outcome:
                data = (self.root / outcome["path"]).read_bytes()
                if digest(data) != outcome["sha256"]:
                    raise ValueError(f"failed response bytes changed: {outcome['path']}")
                response = history.with_suffix(".response")
                response.parent.mkdir(exist_ok=True)
                response.write_bytes(data)
                outcome["archived_response_path"] = str(response.relative_to(self.root))
            write_json(history, outcome)
            record.unlink()


def semantic_sources(outcomes: list[dict]) -> list[dict]:
    return [{k: o[k] for k in ("path", "url", "retrieved_at", "sha256", "bytes")}
            for o in outcomes if o["status"] == "ok"]


def select_candidates(executable: Path, snapshot: Path, boundary: Path, policy_sha256: str) -> dict:
    executable = executable.resolve()
    binary_hash = digest(executable.read_bytes())
    with tempfile.TemporaryDirectory(prefix="obc-landmark-selection-") as temporary:
        subprocess.run([str(executable), "landmark-content", "--snapshot", str(snapshot), "--boundary", str(boundary), "--out", temporary], check=True)
        if digest(executable.read_bytes()) != binary_hash:
            raise ValueError("compiler changed during selection; retry with a stable executable")
        content = json.loads((Path(temporary) / "content.json").read_text())
        if content["category_policy_sha256"] != policy_sha256:
            raise ValueError("discovery policy differs from compiler policy")
        return dict(compiler_sha256=binary_hash, policy_sha256=content["policy_sha256"], candidate_qids=content["candidate_qids"])


def polygons(boundary: dict) -> list:
    if boundary["type"] == "FeatureCollection":
        return [p for feature in boundary["features"] for p in polygons(feature)]
    if boundary["type"] == "Feature":
        return polygons(boundary["geometry"])
    if boundary["type"] == "Polygon":
        return [boundary["coordinates"]]
    if boundary["type"] == "MultiPolygon":
        return boundary["coordinates"]
    raise ValueError("boundary must contain polygons")


def bbox(boundary: dict) -> tuple[float, float, float, float]:
    points = [point for polygon in polygons(boundary) for ring in polygon for point in ring]
    return min(p[0] for p in points), min(p[1] for p in points), max(p[0] for p in points), max(p[1] for p in points)


def query(root: str, bounds: tuple, *, box: bool = False) -> str:
    west, south, east, north = bounds
    if not re.fullmatch(r"Q[1-9][0-9]*", root):
        raise ValueError("invalid policy root")
    if box:
        return f'''SELECT DISTINCT ?item ?location WHERE {{
  SERVICE wikibase:box {{ ?item wdt:P625 ?location.
    bd:serviceParam wikibase:cornerSouthWest "Point({west} {south})"^^geo:wktLiteral;
      wikibase:cornerNorthEast "Point({east} {north})"^^geo:wktLiteral. }}
  ?item wdt:P31/wdt:P279* wd:{root}.
}}'''
    # Match Wikidata's best-rank semantics. The compiler applies the exact polygon
    # and excluded classes again against the captured raw entity revisions.
    return f'''SELECT DISTINCT ?item ?location WHERE {{
  ?item wdt:P31/wdt:P279* wd:{root}; wdt:P625 ?location.
  FILTER(geof:latitude(?location)>={south} && geof:latitude(?location)<={north}
    && geof:longitude(?location)>={west} && geof:longitude(?location)<={east})
}}'''


def claim_values(entity: dict, property_id: str) -> list:
    claims = entity.get("claims", {}).get(property_id, [])
    rank = "preferred" if any(c.get("rank") == "preferred" for c in claims) else "normal"
    return [claim["mainsnak"]["datavalue"]["value"] for claim in claims
            if claim.get("rank", "normal") == rank and "datavalue" in claim.get("mainsnak", {})]


class LeadImage(HTMLParser):
    """Match the compiler's first mw-file-description link before the first h2."""

    def __init__(self):
        super().__init__()
        self.in_content = False
        self.finished = False
        self.filename = None
        self.pending_filename = None
        self.status = "absent"

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if attrs.get("id") == "mw-content-text":
            self.in_content = True
        if self.in_content and not self.finished and tag == "h2":
            self.finished = True
            self.status = "unsupported-lead"
        if self.in_content and not self.finished and tag == "a" and "mw-file-description" in attrs.get("class", "").split():
            self.finished = True
            path = urlparse(attrs.get("href", "")).path
            if "/wiki/File:" in path:
                self.pending_filename = unquote(path.split("/wiki/File:", 1)[1]).replace("_", " ")
        if tag == "img" and self.pending_filename:
            source = urlparse(attrs.get("src", ""))
            if source.hostname == "upload.wikimedia.org" and source.path.startswith("/wikipedia/commons/"):
                self.filename = self.pending_filename
                self.status = "commons"
            else:
                self.status = "unsupported-repository"
            self.pending_filename = None

    def handle_endtag(self, tag):
        if tag == "a":
            self.pending_filename = None


def entity(capture: Capture, qid: str, directory: str = "entities") -> dict | None:
    raw = capture.json(f"{directory}/{qid}.json", f"https://www.wikidata.org/wiki/Special:EntityData/{qid}.json")
    value = raw.get("entities", {}).get(qid) if raw else None
    if value is None and directory == "classes" and raw:
        # EntityData follows redirects. The API also records the original ID,
        # which is needed to keep aliases in the captured class closure.
        redirected = capture.json(f"classes/{qid}-redirect.json", api("www.wikidata.org", action="wbgetentities", ids=qid, redirects="yes", props="claims"))
        value = redirected.get("entities", {}).get(qid) if redirected else None
    return value


def class_parents(value: dict, qid: str) -> list[str]:
    redirect = value.get("redirects")
    if redirect:
        target = redirect.get("to", "")
        if redirect.get("from") != qid or value.get("id") != target or not re.fullmatch(r"Q[1-9][0-9]*", target):
            raise ValueError(f"invalid class redirect: {qid}")
        return [target]
    return [v["id"] for v in claim_values(value, "P279") if isinstance(v, dict) and "id" in v]


def article(capture: Capture, qid: str, language: str, title: str) -> tuple[dict | None, str | None, str]:
    path = f"articles/{language}-{qid}.json"
    url = api(f"{language}.wikipedia.org", action="query", titles=title, redirects=1, prop="pageprops|revisions", rvprop="ids|timestamp|content", rvslots="main")
    raw = capture.json(path, url)
    if not raw:
        return None, None, "acquisition-failed"
    pages = list(raw.get("query", {}).get("pages", {}).values())
    if len(pages) != 1 or not pages[0].get("revisions"):
        return None, None, "article-missing"
    page = pages[0]
    revision = page["revisions"][0]
    html_path = f"articles/{language}-{qid}.html"
    rendered_url = f"https://{language}.wikipedia.org/w/index.php?" + urlencode({"title": page["title"], "oldid": revision["revid"]})
    rendered = capture.fetch(html_path, rendered_url)
    if rendered["status"] != "ok":
        return None, None, "rendered-acquisition-failed"
    parser = LeadImage()
    parser.feed((capture.root / html_path).read_text())
    record = dict(language=language, title=page["title"], url=rendered_url, revision=revision["revid"], timestamp=revision["timestamp"], path=path, html_path=html_path, attribution_source=html_path, notices_source=path, lead_image_status=parser.status)
    return record, parser.filename, "captured"


def photo(capture: Capture, filename: str) -> tuple[dict | None, str]:
    filename = filename.replace("_", " ")
    key = digest(filename.encode())
    metadata = f"images/{key}.json"
    raw = capture.json(metadata, api("commons.wikimedia.org", action="query", titles="File:" + filename, prop="imageinfo", iiprop="url|timestamp|sha1|extmetadata|mime|size", iilimit=1, uselang="en"))
    if not raw:
        return None, "metadata-acquisition-failed"
    pages = list(raw.get("query", {}).get("pages", {}).values())
    if len(pages) != 1 or not pages[0].get("imageinfo"):
        return None, "not-on-commons"
    info = pages[0]["imageinfo"][0]
    extension = {"image/jpeg": ".jpg", "image/png": ".png"}.get(info.get("mime"))
    if not extension:
        return None, "unsupported-format"
    if info.get("size", 0) > MAX_SOURCE:
        return None, "oversized"
    path = f"images/{key}{extension}"
    result = capture.fetch(path, info["url"])
    if result["status"] != "ok":
        return None, result["status"]
    return dict(path=path, metadata_path=metadata, filename=filename), "captured"


def capture_locales(capture: Capture, value: dict) -> None:
    def ids(raw, prop):
        return {v["id"] for v in claim_values(raw, prop) if isinstance(v, dict) and re.fullmatch(r"Q[1-9][0-9]*", v.get("id", ""))}
    # Countries are fetched independently of the administrative traversal budget.
    for qid in sorted(ids(value, "P17")):
        entity(capture, qid, "locales")
    pending, visited = ids(value, "P131"), set()
    for _ in range(MAX_LOCALE_DEPTH):
        following = set()
        for qid in sorted(pending - visited):
            if len(visited) >= MAX_LOCALES:
                return
            visited.add(qid)
            raw = entity(capture, qid, "locales")
            if raw:
                following.update(ids(raw, "P131"))
        pending = following


def capture_place(capture: Capture, qid: str) -> dict:
    value = entity(capture, qid)
    place = dict(qid=qid, articles=[], images=[], outcomes=[])
    if not value or "missing" in value:
        place["outcomes"].append(dict(asset="entity", status="acquisition-failed"))
        return place
    return capture_assets(capture, qid, value)


def capture_assets(capture: Capture, qid: str, value: dict) -> dict:
    place = dict(qid=qid, articles=[], images=[], outcomes=[])
    place["entity_revision"] = value.get("lastrevid")
    place["name"] = next((value.get("labels", {}).get(lang, {}).get("value") for lang in LANGUAGES if value.get("labels", {}).get(lang)), qid)
    coordinates = claim_values(value, "P625")
    place["coordinate"] = coordinates[0] if coordinates else None
    if not any(lang + "wiki" in value.get("sitelinks", {}) for lang in LANGUAGES):
        place["outcomes"].append(dict(asset="article", status="no-supported-sitelink"))
        return place
    capture_locales(capture, value)
    candidates = {(name.replace("_", " "), "P18", None) for name in claim_values(value, "P18") if isinstance(name, str)}
    for language in LANGUAGES:
        link = value.get("sitelinks", {}).get(language + "wiki")
        if not link:
            place["outcomes"].append(dict(asset="article", language=language, status="no-sitelink"))
            continue
        record, lead, status = article(capture, qid, language, link["title"])
        place["outcomes"].append(dict(asset="article", language=language, status=status))
        if record:
            place["articles"].append(record)
            if not lead:
                place["outcomes"].append(dict(asset="photo", source="wikipedia-lead", language=language, status=record["lead_image_status"]))
        if lead:
            candidates.add((lead, "wikipedia-lead", language))
    if not candidates:
        place["outcomes"].append(dict(asset="photo", status="no-supported-candidate"))
    for name, source, language in sorted(candidates, key=lambda v: (v[1] != "P18", v[0], v[2] or "")):
        image, status = photo(capture, name)
        place["outcomes"].append(dict(asset="photo", source=source, filename=name, language=language, status=status))
        if image:
            place["images"].append(dict(image, source=source, language=language))
    return place


def run(args) -> int:
    boundary_bytes = args.boundary.read_bytes()
    policy_bytes = args.policy.read_bytes()
    boundary, policy = json.loads(boundary_bytes), json.loads(policy_bytes)
    capture = Capture(args.out)
    recipe = dict(schema=1, rank="best-rank", boundary_sha256=digest(boundary_bytes), policy_sha256=digest(policy_bytes), languages=LANGUAGES, locale_policy="P131-P37-depth8-nodes64;P17-P37;ui-order", language_sha256=digest(LANGUAGE_BYTES))
    recipe_path = args.out / "recipe.json"
    if recipe_path.exists() and json.loads(recipe_path.read_text()) != json.loads(json.dumps(recipe)):
        raise ValueError("capture recipe changed; use a new output directory")
    write_json(recipe_path, recipe)
    if args.retry_failed:
        capture.retry_failed()
    (args.out / "boundary.geojson").write_bytes(boundary_bytes)
    (args.out / "policy.json").write_bytes(policy_bytes)
    roots = sorted({root for group in policy["groups"].values() if group["include"] for root in group["roots"]}, key=lambda q: int(q[1:]))
    qids, queries = set(), []
    for root in roots:
        sparql = query(root, bbox(boundary))
        raw = capture.json(f"queries/{root}.json", "https://query.wikidata.org/sparql?" + urlencode({"query": sparql, "format": "json"}))
        rows = raw.get("results", {}).get("bindings") if raw else None
        strategy = "class-first"
        if not isinstance(rows, list):
            strategy = "box-first"
            sparql = query(root, bbox(boundary), box=True)
            raw = capture.json(f"queries/{root}-box.json", "https://query.wikidata.org/sparql?" + urlencode({"query": sparql, "format": "json"}))
            rows = raw.get("results", {}).get("bindings") if raw else None
        queries.append(dict(root=root, strategy=strategy, complete=isinstance(rows, list), rows=len(rows) if isinstance(rows, list) else None))
        if isinstance(rows, list):
            for row in rows:
                qid = row.get("item", {}).get("value", "").rsplit("/", 1)[-1]
                if not re.fullmatch(r"Q[1-9][0-9]*", qid):
                    raise ValueError("invalid Wikidata query identity")
                qids.add(qid)
        print(f"query {root}: {queries[-1]}; union={len(qids)}", flush=True)
    ordered = sorted(qids, key=lambda q: int(q[1:]))
    write_json(args.out / "candidates.json", dict(qids=ordered, queries=queries))
    places = []
    with ThreadPoolExecutor(max_workers=2) as pool:
        for qid, value in zip(ordered, pool.map(lambda q: entity(capture, q), ordered)):
            if value and "missing" not in value:
                places.append(dict(qid=qid, entity_revision=value.get("lastrevid"), articles=[], images=[]))
            print(f"entity {qid}: {'captured' if value else 'failed'}", flush=True)
    classes = {}
    pending = set(roots) | set(policy["exclude_roots"])
    for place in places:
        raw = entity(capture, place["qid"])
        if raw:
            pending.update(v["id"] for v in claim_values(raw, "P31") if isinstance(v, dict) and "id" in v)
    missing = []
    while pending:
        qid = min(pending, key=lambda q: int(q[1:]))
        pending.remove(qid)
        value = entity(capture, qid, "classes")
        if not value or "missing" in value:
            missing.append(qid)
            classes[qid] = []
            continue
        parents = class_parents(value, qid)
        classes[qid] = parents
        pending.update(p for p in parents if p not in classes)
        if len(classes) + len(pending) > MAX_CLASSES:
            raise ValueError("class closure exceeds bound")
    def manifest(asset_phase_complete):
        outcomes = capture.outcomes()
        failures = [o for o in outcomes if o["status"] != "ok"]
        unresolved = [o for o in failures if not o["path"].startswith("queries/")]
        entities_complete = len(places) == len(qids)
        coverage = dict(kind="geographic-policy-superset", country_complete=asset_phase_complete and entities_complete and all(q["complete"] for q in queries) and not unresolved and not missing,
                    query_coverage_complete=all(q["complete"] for q in queries), bbox=bbox(boundary), boundary_path="boundary.geojson", policy_path="policy.json",
                    queries=queries, candidate_identities=len(qids), acquired_entities=sum("entity_revision" in p for p in places), entity_coverage_complete=entities_complete,
                    asset_phase_complete=asset_phase_complete, request_failures=len(failures), unresolved_source_failures=len(unresolved), missing_classes=missing,
                    selection="Best-rank P31/P279 policy-root union in geographic bounding box; compiler applies exact polygon, best-rank claims and exclusions. No P17 constraint.")
        write_json(args.out / "manifest.json", dict(schema=1, sources=semantic_sources(outcomes), places=places, classes_path="classes.json", missing_classes=missing, coverage=coverage, outcomes=outcomes))
        return coverage
    write_json(args.out / "classes.json", classes)
    manifest(False)
    # The production compiler owns geometry and category selection. Acquisition
    # does not maintain a second implementation of that policy.
    selection = select_candidates(args.select_with, args.out / "manifest.json", args.boundary, digest(policy_bytes))
    selected = selection["candidate_qids"]
    write_json(args.out / "selection.json", selection)
    selected_set = set(selected)
    with ThreadPoolExecutor(max_workers=2) as pool:
        def work(qid):
            result = capture_place(capture, qid)
            write_json(args.out / "places" / (qid + ".json"), result)
            return result
        captured = {}
        for place in pool.map(work, [q for q in ordered if q in selected_set]):
            captured[place["qid"]] = place
            print(f"place {len(captured)}/{len(selected)}: {place['qid']} articles={len(place['articles'])} images={len(place['images'])}", flush=True)
        places = [captured.get(p["qid"], dict(p, outcomes=[dict(asset="site", status="excluded-by-production-selection")])) for p in places]
    coverage = manifest(True)
    print(json.dumps(coverage, indent=2), flush=True)
    return 0 if coverage["country_complete"] else 2


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--boundary", required=True, type=Path)
    parser.add_argument("--policy", type=Path, help="required for landmark policy discovery")
    parser.add_argument("--peaks-osm", type=Path, help="capture a separate peak catalogue from this regional OSM extract")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--select-with", required=True, type=Path, help="built obc-bake binary; it selects entities before asset acquisition")
    parser.add_argument("--retry-failed", action="store_true", help="retry failed requests once, retaining their previous outcomes")
    args = parser.parse_args()
    try:
        if args.peaks_osm:
            from peak_capture import run as run_peaks
            return run_peaks(args)
        if args.policy is None:
            raise ValueError("landmarks require --policy")
        return run(args)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"landmark capture: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
