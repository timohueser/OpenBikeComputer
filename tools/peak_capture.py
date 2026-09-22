"""Peak collection in the shared immutable Wiki acquisition pipeline."""
from pathlib import Path
import json
import re
import subprocess

if __package__:
    from . import landmark_capture as shared
else:
    import landmark_capture as shared


def wikipedia(value):
    language, separator, title = value.partition(":")
    if separator and re.fullmatch(r"[a-z][a-z-]{1,11}", language) and title.strip() and not title.startswith("//"):
        return language, title
    return None


def resolve(capture, summit):
    """One outcome per explicit OSM tag; no name or location matching."""
    results, identities = [], {}
    for kind in ("wikidata", "wikipedia"):
        value = summit["tags"].get(kind)
        if value is None:
            continue
        result = dict(node_id=summit["node_id"], kind=kind, status="invalid_explicit_link")
        results.append(result)
        if kind == "wikidata":
            if not re.fullmatch(r"Q[1-9][0-9]*", value):
                continue
            path = f"links/wikidata-{value}.json"
            url = shared.api("www.wikidata.org", action="wbgetentities", ids=value, redirects="yes", props="info")
        else:
            link = wikipedia(value)
            if not link:
                continue
            language, title = link
            path = f"links/wikipedia-{shared.digest(value.encode())}.json"
            url = shared.api(f"{language}.wikipedia.org", action="query", titles=title, redirects=1, prop="pageprops|langlinks", lllimit="max")
        raw = capture.json(path, url)
        result["path"] = path
        if raw is None:
            result["status"] = "link_acquisition_failed"
            continue
        if kind == "wikidata":
            item = raw.get("entities", {}).get(value, {})
            canonical = item.get("id", "")
            valid = re.fullmatch(r"Q[1-9][0-9]*", canonical) and "missing" not in item
            if canonical != value:
                valid = valid and item.get("redirects") == {"from": value, "to": canonical}
            if not valid:
                result["status"] = "wikidata_resolution_failed"
                continue
            identities[canonical] = None
        else:
            pages = list(raw.get("query", {}).get("pages", {}).values())
            if len(pages) != 1 or pages[0].get("pageid", 0) <= 0 or "missing" in pages[0]:
                result["status"] = "wikipedia_resolution_failed"
                continue
            page = pages[0]
            canonical = page.get("pageprops", {}).get("wikibase_item")
            if canonical and re.fullmatch(r"Q[1-9][0-9]*", canonical):
                identities[canonical] = None
            else:
                if "continue" in raw:
                    result["status"] = "incomplete_language_links"
                    continue
                links = {item["lang"]: item["*"] for item in page.get("langlinks", [])}
                links[language] = page["title"]
                supported = {code: links[code] for code in shared.LANGUAGES if code in links}
                if not supported:
                    result["status"] = "no_supported_language_link"
                    continue
                selected = next(iter(supported))
                canonical_page = page
                canonical_raw = raw
                if selected != language:
                    canonical_path = f"links/wikipedia-{shared.digest((selected + ':' + supported[selected]).encode())}.json"
                    canonical_raw = capture.json(canonical_path, shared.api(f"{selected}.wikipedia.org", action="query", titles=supported[selected], redirects=1, prop="pageprops|langlinks", lllimit="max"))
                    canonical_pages = list((canonical_raw or {}).get("query", {}).get("pages", {}).values())
                    if len(canonical_pages) != 1 or canonical_pages[0].get("pageid", 0) <= 0:
                        result["status"] = "language_link_acquisition_failed"
                        continue
                    canonical_page = canonical_pages[0]
                    result["canonical_path"] = canonical_path
                result["canonical_language"] = selected
                canonical = canonical_page.get("pageprops", {}).get("wikibase_item")
                if canonical and re.fullmatch(r"Q[1-9][0-9]*", canonical):
                    identities[canonical] = None
                else:
                    if "continue" in canonical_raw:
                        result["status"] = "incomplete_language_links"
                        continue
                    links = {item["lang"]: item["*"] for item in canonical_page.get("langlinks", [])}
                    links[selected] = canonical_page["title"]
                    supported = {code: links[code] for code in shared.LANGUAGES if code in links}
                    canonical = f"wiki-{selected}-{canonical_page['pageid']}"
                    identities[canonical] = dict(id=canonical, labels={code: dict(value=title) for code, title in supported.items()}, sitelinks={code + "wiki": dict(title=title) for code, title in supported.items()})
        result["status"] = "resolved"
    return results, identities


def run(args):
    capture = shared.Capture(args.out)
    boundary_bytes = args.boundary.read_bytes()
    # The production classifier owns discovery, including exact boundary selection.
    candidate_path = args.out / "summits.json"
    with shared.tempfile.TemporaryDirectory(prefix="obc-peak-discovery-") as temporary:
        generated = Path(temporary) / "summits.json"
        subprocess.run([str(args.select_with.resolve()), "peak-candidates", "--osm", str(args.peaks_osm), "--boundary", str(args.boundary), "--out", str(generated)], check=True)
        candidate_bytes = generated.read_bytes()
    recipe = dict(schema=1, collection="peaks", summits_sha256=shared.digest(candidate_bytes), boundary_sha256=shared.digest(boundary_bytes), language_sha256=shared.digest(shared.LANGUAGE_BYTES))
    recipe_path = args.out / "recipe.json"
    if recipe_path.exists() and json.loads(recipe_path.read_text()) != recipe:
        raise ValueError("capture recipe changed; use a new output directory")
    shared.write_json(recipe_path, recipe)
    candidate_path.write_bytes(candidate_bytes)
    (args.out / "boundary.geojson").write_bytes(boundary_bytes)
    if args.retry_failed:
        capture.retry_failed()
    summits = json.loads(candidate_bytes)["summits"]
    resolutions, identities = [], {}
    for summit in summits:
        resolved, found = resolve(capture, summit)
        resolutions.extend(resolved)
        if len(found) == 1:
            identities.update(found)
    places = []
    for index, (identity, direct) in enumerate(sorted(identities.items())):
        place = shared.capture_place(capture, identity, True) if direct is None else shared.capture_assets(capture, identity, direct, True)
        places.append(place)
        print(f"peak article {index + 1}/{len(identities)}: {identity} articles={len(place['articles'])} images={len(place['images'])}", flush=True)
    def manifest():
        outcomes = capture.outcomes()
        sources = shared.semantic_sources(outcomes)
        sources.append(dict(path="summits.json", url="urn:openbikecomputer:osm-summits:" + json.loads(candidate_bytes)["osm_sha256"], bytes=len(candidate_bytes), sha256=shared.digest(candidate_bytes)))
        sources.sort(key=lambda source: source["path"])
        failures = [item for item in outcomes if item["status"] != "ok"]
        coverage = dict(kind="osm-named-summits", country_complete=False, asset_phase_complete=True, named_summits=len(summits), linked_summits=sum(bool(s["tags"].get("wikidata") or s["tags"].get("wikipedia")) for s in summits), canonical_articles=len(identities), request_failures=len(failures), selection="Explicit OSM wikidata/wikipedia links from the map summit classifier; no inferred matches.")
        shared.write_json(args.out / "manifest.json", dict(schema=1, sources=sources, places=places, peaks=dict(summits_path="summits.json", resolutions=resolutions), coverage=coverage, outcomes=outcomes))
        return coverage, failures
    shared.acquire_requested_photos(capture, args.select_with, "peaks", "peaks.json", args.out / "manifest.json", args.boundary, places, manifest)
    coverage, failures = manifest()
    print(json.dumps(coverage, indent=2), flush=True)
    return 2 if failures else 0
