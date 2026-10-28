"""The TrailView ruleset of data/trailview/: its checks, a port of `validate()` in the prototype's
`src/regions.ts`, and the bundle that specs/trailview-tiles.md defines.

    uv run --locked --group trailview python -m tools.trailview.rules     # print each problem

The `obc data` step `planner/trailview-rules` (`--step`) checks the ruleset and writes `rules.json`
for the engine, and `regions.json`, `materials.json` and `ground.json` for the ground tiles.

A region id is a region and a place: the region's own id, then one id per country or subdivision
that a value by place in its chain names.
"""

import hashlib
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RULESET = ROOT / "data/trailview"
FORMAT = 3

# WorldCover's groups in the order of the prototype's ramps (`src/materials.ts`).
COVER_ORDER = ("tree", "shrub", "grass", "crop", "built", "bare", "snow", "water", "wetland", "mangrove", "moss")
SEASONS = ("winter", "spring", "summer", "autumn")
RAMP_COVERS = (*(cover for cover in COVER_ORDER if cover != "tree"), "needle", "broad")
PLACE = re.compile(r"^([a-z]{2}(-[a-z0-9]{1,3})?|\*)$")
# A region id is one byte of the region tiles, and 255 is no data there.
MAX_REGION_IDS = 255
# A material code is 6 bits of the ground tiles (specs/trailview-tiles.md#ground).
MAX_MATERIALS = 64
# The patches of a region's look that the ground draws (PATCHES of ground/rules.ts).
MAX_PATCHES = 2
# The tree species of a region that the engine's forest holds (SPECIES of vegetation/forest.ts), its
# ground plants and its shrubs that the plants hold (SLOTS of vegetation/plants.ts), and the engine's
# models: one file per tree and per plant, named by its id.
MAX_TREE_SPECIES = 32
MAX_PLANT_SLOTS = 16
# The house styles that the style table of structures/houses.ts holds: STYLES of 64, the last the industrial hall's.
MAX_STYLES = 63
MODELS = ROOT / "builder/web/src/lib/trailview/models"


# A check takes a value and its key path, and returns its problems, each "key.path: what is wrong".
def said(v):
    return json.dumps(v, separators=(",", ":"), ensure_ascii=False)


def wrong(path, what):
    return [f"{path}: {what}" if path else what]


def is_number(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def number(low, high):
    return lambda v, p: [] if is_number(v) and low <= v <= high else wrong(p, f"{said(v)} is not a number from {low} to {high}")


def positive(v, p):
    return [] if is_number(v) and v > 0 else wrong(p, f"{said(v)} is not a number above 0")


def text(v, p):
    return [] if isinstance(v, str) and v else wrong(p, f"{said(v)} is not text")


def one_of(*values):
    return lambda v, p: ([] if any(type(v) is type(x) and v == x for x in values)
                         else wrong(p, f"{said(v)} is not one of {', '.join(map(said, values))}"))


def colour(v, p):
    return [] if isinstance(v, str) and re.fullmatch(r"#[0-9a-fA-F]{6}", v) else wrong(p, f"{said(v)} is not a #rrggbb colour")


def listed(item, length=None):
    def check(v, p):
        if isinstance(v, list) and (len(v) > 0 if length is None else len(v) == length):
            return [problem for i, x in enumerate(v) for problem in item(x, f"{p}[{i}]")]
        return wrong(p, f"{said(v)} is not a list of {length or 'one or more'}")
    return check


def obj(table):
    """An object whose keys each have their check; any other key is unknown."""
    def check(v, p):
        if not isinstance(v, dict):
            return wrong(p, f"{said(v)} is not an object")
        at = lambda key: f"{p}.{key}" if p else key
        return [problem for key, x in v.items() for problem in (table[key](x, at(key)) if key in table else wrong(at(key), "is an unknown key"))]
    return check


def each(names, check):
    return obj({name: check for name in names})


def entries(entry):
    """Entries keyed by name, such as a species, where null drops the parent's."""
    return lambda v, p: (wrong(p, f"{said(v)} is not an object") if not isinstance(v, dict)
                         else [problem for key, x in v.items() if x is not None for problem in entry(x, f"{p}.{key}")])


def by_place(check):
    """A value, or a map by country or subdivision code with a "*" entry."""
    def by(v, p):
        if not isinstance(v, dict):
            return check(v, p)
        return [*([] if "*" in v else wrong(p, 'has no "*" entry')),
                *(problem for key, x in v.items() for problem in
                  (check(x, f"{p}.{key}") if PLACE.match(key) else wrong(f"{p}.{key}", 'is not a country or subdivision code or "*"')))]
    return by


def point(v, p):
    return [] if isinstance(v, list) and len(v) == 2 and all(map(is_number, v)) else wrong(p, f"{said(v)} is not a [lon, lat]")


def area(v, p):
    """A box [west, south, east, north] or a ring of three or more [lon, lat]."""
    if isinstance(v, list) and len(v) == 4 and all(map(is_number, v)):
        w, s, e, n = v
        return [] if w < e and s < n else wrong(p, f"{said(v)}: west is not below east, or south below north")
    if isinstance(v, list) and len(v) >= 3 and all(isinstance(x, list) for x in v):
        return listed(point)(v, p)
    return wrong(p, f"{said(v)} is not a box [west, south, east, north] or a ring of 3 or more [lon, lat]")


def ground_names(materials):
    """A list of ground: each entry a cover, a material or "#tag" of `materials`."""
    known = set(COVER_ORDER) | set(materials["materials"]) | {f"#{tag}" for tag in materials["tags"]}
    return listed(lambda v, p: [] if isinstance(v, str) and v in known
                  else wrong(p, f"{said(v)} is not a cover, a material or a #tag of materials.json"))


def patch(v, p):
    """A whole patch of a region's look, after its chain merged it. The far nodes draw without the dots
    (DETAIL of ground/look.wgsl): with bushes up to 3 m and up to 64° N, the dots fade out before them.
    Blotches have no such fade, so their fine noise, size_m across, must span two pixels where the far
    nodes start: 2 × 0.37 m (the output pixel there that DETAIL of ground/look.wgsl names)."""
    if missing := [key for key in ("on", "colour", "share", "size_m", "form") if key not in v]:
        return wrong(p, f"has no {', '.join(missing)}")
    if v["form"] == "dots" and v["size_m"] > 3:
        return wrong(f"{p}.size_m", f"{said(v['size_m'])} is above 3: the dots of larger bushes stop where the far nodes start")
    if v["form"] == "blotches" and v["size_m"] < 2 * 0.37:
        return wrong(f"{p}.size_m", f"{said(v['size_m'])} is below 0.74: finer blotches shimmer before the far nodes start")
    return []


def tables(materials):
    """The key tables of the region, pack and style files, which name the materials and tags of `materials`."""
    ramp = listed(colour, 3)
    weather = obj({
        "cover": number(0, 1), "base": number(0, 9000), "dark": number(0, 1), "sun": number(0, 2), "ambient": number(0, 2),
        "grey": number(0, 1), "zenith": colour, "horizon": colour, "haze": number(0, 2), "wet": number(0, 1),
        "rain": number(0, 1), "towers": number(0, 1),
    })
    paint = lambda v, p: colour(v, p) if isinstance(v, str) else each(SEASONS, colour)(v, p)
    look = obj({
        "snap": number(0, 1), "hazeKm": number(1, 500), "hazeHeight": number(100, 20_000), "mistTop": number(0, 5000),
        "softness": number(0, 1), "gradient": number(0, 1),
        "ramps": each(RAMP_COVERS, lambda v, p: ramp(v, p) if isinstance(v, list) else each(SEASONS, ramp)(v, p)),
        "patches": entries(obj({"on": ground_names(materials), "colour": paint, "share": number(0, 1), "size_m": number(0.1, 100),
                                "form": one_of("blotches", "dots")})),
        "turf": one_of(*materials["materials"]),
        "materials": each(materials["materials"], obj({"colour": paint, "satellite": number(0, 1), "stones": number(0, 1),
                                                       "clumps": number(0, 1)})),
        "lithology": each(materials["lithologies"][1:], obj({"colour": colour, "banded": one_of(True)})),
        "bare_luma": lambda v, p: listed(number(0, 1), 4)(v, p) if isinstance(v, list) else number(0, 1)(v, p),
        "outcrops": number(0, 1), "turf_deg": number(0, 90), "rock": colour, "cliff": colour,
        "snow_share": number(0, 1), "bedding": obj({"strike": number(0, 360), "dip": number(0, 90)}), "roads": listed(colour, 6),
        "roadLine": by_place(colour), "sea": obj({"deep": colour, "shallow": colour}), "windswept_m": number(0, 5000),
        "swell": number(0, 360),
        "grade": obj({"shadow": listed(number(-0.1, 0.1), 3), "light": listed(number(-0.1, 0.1), 3), "saturation": number(0, 3)}),
        "weather": each(("sunny", "cloudy", "rainy"), weather),
    })
    region = obj({
        "name": text, "about": text, "ecoregion": number(1, 1e6), "extends": text, "where": listed(area),
        "leaf": one_of("needleleaved", "broadleaved", "mixed"), "pack": text,
        "look": look,
    })
    species = obj({
        "model": text, "latin": text, "leaf": one_of("needleleaved", "broadleaved"), "cover": one_of("tree", "shrub"),
        "elevation": listed(number(-500, 9000), 2), "weight": positive, "evergreen": one_of(True, False),
    })
    ground = obj({
        "model": text, "latin": text, "on": ground_names(materials), "off": ground_names(materials), "height": listed(number(0, 50), 2),
        "share": number(0, 1), "own": one_of(True), "clumped": one_of(True, False),
    })
    herd = obj({"model": text, "latin": text, "groups": number(1, 100), "size": listed(number(1, 1000), 2),
                "on": ground_names(materials)})
    pack = obj({
        "name": text, "about": text, "extends": text, "trees": entries(species), "ground": entries(ground),
        "herds": entries(herd), "style": by_place(text), "houses": text, "orchard": text,
    })
    style = obj({
        "name": text, "about": text, "extends": text, "pitch": listed(number(0, 80), 2), "eave": number(0, 5), "verge": number(0, 5),
        "hip": number(0, 1), "timber": one_of("none", "upper", "all", "frame"), "shutters": one_of(True, False),
        "balconies": one_of(True, False), "dormers": one_of(True, False), "chimneys": one_of(0, 1, 2), "spire": one_of("onion", "domes"),
        "windows": number(0.3, 1.5), "storeys": number(1, 8), "roof": one_of("cone", "flat"),
        "palette": obj({"render": listed(colour), "wood": listed(colour), "roof": listed(colour), "door": listed(colour),
                        "shutter": listed(colour), "plinth": lambda v, p: (listed(colour) if isinstance(v, list) else colour)(v, p),
                        "chimney": colour}),
    })
    return region, pack, style


def files(root=RULESET):
    """Every file of the ruleset that the step reads, by its path relative to `root`."""
    return sorted(path.relative_to(root).as_posix() for path in root.rglob("*.json"))


def digest(root=RULESET):
    """The SHA-256 of the `sha256sum` text of `files`: the `ruleset` option of the step."""
    lines = "".join(f"{hashlib.sha256((root / name).read_bytes()).hexdigest()}  {name}\n" for name in files(root))
    return hashlib.sha256(lines.encode()).hexdigest()


def read(root, name):
    """A JSON file of the ruleset. Python reads Infinity and NaN, which JSON does not have."""
    def constant(text):
        raise ValueError(f"{name}: {text} is not JSON")
    return json.loads((root / name).read_text(), parse_constant=constant)


def load(root=RULESET):
    """The ruleset: the vocabulary, the ecoregions, the places and each file of the folders by name."""
    folder = lambda name: {path.stem: read(root, path.relative_to(root).as_posix()) for path in sorted((root / name).glob("*.json"))}
    vocabulary, places = read(root, "materials.json"), read(root, "countries.json")
    return {
        "materials": vocabulary, "ground": read(root, "ground.json"), "ecoregions": read(root, "ecoregions.json")["regions"],
        "places": places["countries"] + places["subdivisions"],
        "regions": folder("regions"), "packs": folder("packs"), "styles": folder("styles"),
    }


def vocabulary_problems(vocabulary):
    out = []
    if len(vocabulary["materials"]) > MAX_MATERIALS:
        out.append(f"materials.json: {len(vocabulary['materials'])} materials, more than the {MAX_MATERIALS} that a code of the ground tiles holds")
    for i, entry in enumerate(vocabulary["materials"]):
        if entry["cover"] not in COVER_ORDER:
            out.append(f"materials.json: materials[{i}].cover: {entry['cover']} is not one of {', '.join(COVER_ORDER)}")
        out += [f"materials.json: {problem}" for share in ("stones", "clumps", "shrubs") if share in entry
                for problem in number(0, 1)(entry[share], f"materials[{i}].{share}")]
        if "tags" in entry:
            tag = lambda v, p: [] if isinstance(v, str) and re.fullmatch(r"[a-z]+(-[a-z]+)*", v) else wrong(p, f"{said(v)} is not a tag")
            out += [f"materials.json: {problem}" for problem in listed(tag)(entry["tags"], f"materials[{i}].tags")]
    for key in ("materials", "lithologies"):
        ids = [entry["id"] for entry in vocabulary[key]]
        out += [f"materials.json: {key}[{i}].id: {id_} is also {key}[{ids.index(id_)}]" for i, id_ in enumerate(ids) if ids.index(id_) != i]
    return out


# The keys that each protocol of a ground source reads.
NEEDS = {"vector": ("field",), "arcgis-features": ("field",), "wfs": ("layer", "field", "sort"),
         "arcgis-image": ("layer", "crs", "px_m")}


def ground_problems(ground, ids):
    """The problems of `ground.json`: each source a row of data/sources.toml, read by a protocol that it
    can be fetched for, and each name one of the vocabulary."""
    from . import sources
    from .. import data_registry

    def source(v, p):
        return [] if v in data_registry.SOURCES else wrong(p, f"{said(v)} is not a source of data/sources.toml")

    def needs(keys, check):
        return lambda v, p: check(v, p) + [f"{p}: has no {key}" for key in keys if isinstance(v, dict) and key not in v]

    def lower(v, p):
        return text(v, p) or ([] if v == v.lower() else wrong(p, f"{said(v)} is not lowercase, as a word matches"))

    def legend(name):
        def check(v, p):
            match = v.get("match") if isinstance(v, dict) else None
            native = {"tags": lambda x, q: [] if isinstance(x, dict) and x and all(isinstance(y, str) for y in x.values())
                      else wrong(q, f"{said(x)} is not an object of tags"), "word": lower}.get(match, text)
            pair = lambda x, q: (native(x[0], f"{q}[0]") + name(x[1], f"{q}[1]") if isinstance(x, list) and len(x) == 2
                                 else wrong(q, f"{said(x)} is not a [native class, name] pair"))
            qualifier = needs(("word", "within", "class"), obj({"word": lower, "within": number(0, 100), "class": name}))
            return needs(("match", "classes"), obj({
                "match": one_of(*sources.MATCHES), "classes": listed(pair),
                **({"qualifier": qualifier} if match == "word" else {})}))(v, p)
        return check

    out = []
    material, lithology = one_of(*ids["materials"]), one_of(*ids["lithologies"])
    heath = needs(("material", "on", "green_red", "ndvi", "peak", "spring_green_red", "spring_on"), obj({
        "source": source, "rule": one_of("satellite-heath"), "about": text, "material": material, "on": listed(material),
        "green_red": positive, "ndvi": number(-1, 1), "peak": number(0, 2), "spring_green_red": positive, "spring_on": listed(lithology)}))
    out += obj({"about": text, "material": lambda v, p: [], "lithology": lambda v, p: []})(ground, "")
    for layer, name, names in (("material", material, "materials"), ("lithology", lithology, "lithologies")):
        entry = needs(("source", "protocol", "combine", "legend"), obj({
            "source": source, "protocol": one_of(*sources.PROTOCOLS), "layer": text, "field": text, "sort": text, "crs": text,
            "px_m": positive,
            "combine": one_of(*sources.COMBINES), "reach_m": positive, "about": text, "legend": legend(name),
            "zoom": number(10, 13)}))
        spec = ground.get(layer, {})
        out += obj({"about": text, "refines": each(ids[names], listed(name)),
                    "sources": lambda v, p: []})(spec, layer)
        for i, found in enumerate(spec.get("sources", [])):
            at = f"{layer}.sources[{i}]"
            out += (heath if layer == "material" and "rule" in found else entry)(found, at)
            protocol, fetch = found.get("protocol"), data_registry.SOURCES.get(found.get("source"), {}).get("fetch", {})
            for key in NEEDS.get(protocol, ()):
                if key not in found:
                    out.append(f"{at}: has no {key}, which the protocol {protocol} needs")
            zoomed = layer == "lithology" and protocol in sources.VECTOR
            if zoomed and "zoom" not in found:
                out.append(f"{at}: has no zoom")
            if "zoom" in found and not zoomed:
                out.append(f"{at}.zoom: only a vector lithology source has a zoom")
            if "reach_m" in found and protocol not in sources.VECTOR:
                out.append(f"{at}.reach_m: only a vector source reaches")
            if protocol in sources.CAPTURED and fetch.get("kind") != "capture":
                out.append(f"{at}.protocol: {protocol} is captured, and {found['source']} is not a capture source")
    return [f"ground.json: {problem}" for problem in out]


def validate(ruleset):
    """Every problem of the ruleset, one line each: the file, the key path and what is wrong."""
    vocabulary = ruleset["materials"]
    out = vocabulary_problems(vocabulary)
    ids = {key: [entry["id"] for entry in vocabulary[key]] for key in ("materials", "lithologies")}
    ids["tags"] = {tag for entry in vocabulary["materials"] if isinstance(entry.get("tags"), list)
                   for tag in entry["tags"] if isinstance(tag, str)}
    out += ground_problems(ruleset["ground"], ids)
    region_check, pack_check, style_check = tables(ids)
    regions, packs, styles = ruleset["regions"], ruleset["packs"], ruleset["styles"]
    add = lambda file, problems: out.extend(f"{file}: {problem}" for problem in problems)

    # Keys and types, then each file's parent: it exists and the chain does not loop.
    for name, data in regions.items():
        add(f"regions/{name}.json", region_check(data, ""))
    for name, data in packs.items():
        add(f"packs/{name}.json", pack_check(data, ""))
    for name, data in styles.items():
        add(f"styles/{name}.json", style_check(data, ""))
    for kind, table in (("region", regions), ("pack", packs), ("style", styles)):
        for name, data in table.items():
            file, up = f"{kind}s/{name}.json", data.get("extends")
            if kind == "region" and "." in name and "where" not in data:
                add(file, ["where: a subregion needs its areas"])
            if up is None:
                continue
            if up not in table:
                add(file, [f"extends: no {kind}s/{up}.json"])
            loop, at = [name], up
            while isinstance(at, str) and at in table and len(loop) <= len(table):
                loop.append(at)
                if at == name:
                    add(file, [f"extends: a loop {' › '.join(loop)}"])
                    break
                at = table[at].get("extends")

    if len(styles) > MAX_STYLES:
        out.append(f"styles/: {len(styles)} house styles, more than the {MAX_STYLES} that the engine's style table holds")
    # The packs' trees and the places resolve through the chains, which must be sound first.
    return out or model_problems(regions, packs) + patch_problems(regions) + place_problems(ruleset)


def chain(table, name):
    """A file and the files that it extends, nearest first."""
    out = []
    while name in table:
        out.append(table[name])
        name = table[name].get("extends")
    return out


def model_problems(regions, packs, models=MODELS):
    """The species of each region's pack, with what it inherits: MAX_TREE_SPECIES trees and
    MAX_PLANT_SLOTS shrubs and ground plants at most, each tree and orchard with a tree model of the
    engine, each shrub with a tree or a plant model, each ground plant with a plant model."""
    trees_have, plants_have = ({path.stem for path in (models / kind).glob("*.ts")} for kind in ("trees", "plants"))
    out = []
    for name in sorted(regions):
        pack = next((data["pack"] for data in chain(regions, name) if "pack" in data), None)
        file = f"regions/{name}.json: pack {pack}"
        tables = {"trees": {}, "ground": {}}
        for data in reversed(chain(packs, pack)):
            for table, entries in tables.items():
                for species, rule in (data.get(table) or {}).items():
                    if rule is None:
                        entries.pop(species, None)
                    else:
                        entries[species] = {**entries.get(species, {}), **rule}
        own = {species: rule for species, rule in tables["trees"].items() if rule.get("cover") == "tree"}
        if len(own) > MAX_TREE_SPECIES:
            out.append(f"regions/{name}.json: pack {pack} has {len(own)} tree species, more than the {MAX_TREE_SPECIES} that the forest holds")
        shrubs = [species for species, rule in tables["trees"].items() if rule.get("cover") == "shrub"]
        for kind, names in (("shrubs", shrubs), ("ground plants", tables["ground"])):
            if len(names) > MAX_PLANT_SLOTS:
                out.append(f"regions/{name}.json: pack {pack} has {len(names)} {kind}, more than the {MAX_PLANT_SLOTS} that the plants hold")
        for species, rule in tables["trees"].items():
            model = rule.get("model", species)
            if model not in trees_have and not (rule.get("cover") == "shrub" and model in plants_have):
                out.append(f"{file}: {'shrub' if rule.get('cover') == 'shrub' else 'tree'} {species} has no model {model}")
        out += [f"{file}: ground plant {species} has no model {rule.get('model', species)}"
                for species, rule in tables["ground"].items() if rule.get("model", species) not in plants_have]
        orchard = next((data["orchard"] for data in chain(packs, pack) if "orchard" in data), None)
        if isinstance(orchard, str) and orchard not in trees_have:
            out.append(f"{file}: orchard has no model {orchard}")
    return out


def patch_problems(regions):
    """The patches of each region with what it inherits: MAX_PATCHES at most, each whole."""
    out = []
    for name in sorted(regions):
        file = f"regions/{name}.json"
        found = {key: value for key, value in (merged(regions, name).get("look", {}).get("patches") or {}).items() if value is not None}
        if len(found) > MAX_PATCHES:
            out.append(f"{file}: look.patches: {len(found)} patches, more than the {MAX_PATCHES} that the ground draws")
        out += [f"{file}: {problem}" for key, value in found.items() for problem in patch(value, f"look.patches.{key}")]
    return out


def merged(table, name):
    """A file with the files that it extends merged in: objects merge, anything else replaces."""
    def merge(a, b):
        return {**a, **{key: merge(a[key], value) if isinstance(value, dict) and isinstance(a.get(key), dict) else value
                        for key, value in b.items()}}
    out = {}
    for data in reversed(chain(table, name)):
        out = merge(out, data)
    return out


def region_ids(ruleset):
    """Each region id as [region, place]: per region by name, "*" and then each country or
    subdivision that its pack's `style` or its `look.roadLine` names."""
    out = []
    for name in sorted(ruleset["regions"]):
        region = merged(ruleset["regions"], name)
        pack = merged(ruleset["packs"], region["pack"]) if "pack" in region else {}
        places = {key for value in (pack.get("style"), region.get("look", {}).get("roadLine")) if isinstance(value, dict) for key in value}
        out += [[name, place] for place in ["*", *sorted(places - {"*"})]]
    return out


def place_problems(ruleset):
    ids = region_ids(ruleset)
    known = {place["iso"] for place in ruleset["places"]}
    out = [f"regions/{name}.json: {place} is not a place of countries.json" for name, place in ids if place != "*" and place not in known]
    if len(ids) > MAX_REGION_IDS:
        out.append(f"regions/: {len(ids)} region ids, more than the {MAX_REGION_IDS} that one byte of the region tiles holds")
    return out


def bundle(ruleset):
    """`rules.json` and `regions.json`: a region id is its index in `ids`."""
    plain = lambda data: {key: value for key, value in data.items() if key not in ("about", "where")}
    ids = region_ids(ruleset)
    rules = {
        "format": FORMAT, "materials": ruleset["materials"], "ids": ids,
        "regions": {name: plain(data) for name, data in ruleset["regions"].items()},
        "packs": {name: plain(data) for name, data in ruleset["packs"].items()},
        "styles": {name: plain(data) for name, data in ruleset["styles"].items()},
    }
    regions = {
        "format": FORMAT, "ids": ids,
        "regions": {name: {key: data[key] for key in ("extends", "where") if key in data} for name, data in ruleset["regions"].items()},
        "ecoregions": [{"short": e["short"], "polygons": e["polygons"]} for e in ruleset["ecoregions"] if e.get("short") in ruleset["regions"]],
        "places": ruleset["places"],
    }
    return rules, regions


def encoded(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False) + "\n").encode()


def step():
    from .. import step_request

    request = step_request.read()
    if (found := digest()) != request["options"]["ruleset"]:
        raise ValueError(f"data/trailview changed since the plan: digest {found}")
    ruleset = load()
    if problems := validate(ruleset):
        raise ValueError("\n".join([*problems, f"{len(problems)} problems in data/trailview"]))
    output = Path(request["output"])
    rules, regions = bundle(ruleset)
    for name, value in (("rules.json", rules), ("regions.json", regions), ("materials.json", ruleset["materials"]),
                        ("ground.json", ruleset["ground"])):
        (output / name).write_bytes(encoded(value))
    step_request.metrics(request, {"region_ids": len(rules["ids"]), "rules_bytes": (output / "rules.json").stat().st_size})


def main():
    problems = validate(load())
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        sys.exit(f"{len(problems)} problems")
    print(f"data/trailview: no problems, ruleset {digest()}")


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
