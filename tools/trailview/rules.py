"""The TrailView ruleset of data/trailview/: its checks, a port of `validate()` in the prototype's
`src/regions.ts`, and the bundle that specs/trailview-tiles.md defines.

    uv run --locked --group trailview python -m tools.trailview.rules     # print each problem

The `obc data` step `planner/trailview-rules` (`--step`) checks the ruleset and writes `rules.json`
for the engine, and `regions.json` and `materials.json` for the ground tiles.
"""

import hashlib
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RULESET = ROOT / "data/trailview"
FORMAT = 1

# WorldCover's groups in the order of the prototype's ramps (`src/materials.ts`).
COVER_ORDER = ("tree", "shrub", "grass", "crop", "built", "bare", "snow", "water", "wetland", "mangrove", "moss")
# The covers a ground plant stands on (`src/models/library.ts`), and the seasons (`src/models/trees.ts`).
COVERS = ("tree", "shrub", "grass", "crop", "bare", "wetland", "moss")
SEASONS = ("winter", "spring", "summer", "autumn")
RAMP_COVERS = (*(cover for cover in COVER_ORDER if cover != "tree"), "needle", "broad")
PLACE = re.compile(r"^([a-z]{2}(-[a-z0-9]{1,3})?|\*)$")
# The look keys of the engine that overlays/ may set, as a key table like LOOK's below.
OVERLAY_LOOK = {}
# A region id is one byte of the region tiles, and 255 is no data there.
MAX_REGIONS = 255
# The engine's models: one file per tree and per plant, named by its id.
MODELS = ROOT / "builder/web/src/lib/trailview/models"


def models():
    """The ids of the engine's tree and plant models."""
    return ({path.stem for path in (MODELS / "trees").glob("*.ts")}, {path.stem for path in (MODELS / "plants").glob("*.ts")})


def merged(a, b):
    """Objects merge, any other value replaces, as the engine resolves a file that extends another."""
    out = dict(a)
    for key, value in b.items():
        out[key] = merged(a[key], value) if isinstance(value, dict) and isinstance(a.get(key), dict) else value
    return out


def model_problems(packs, trees, plants):
    """The models that a pack names, with what it inherits, and that the engine does not have: a tree of
    `trees`, a shrub of `trees` or `plants`, a ground plant of `plants`, an orchard of `trees`."""
    def resolved(name, seen=()):
        up = packs[name].get("extends")
        base = resolved(up, (*seen, name)) if isinstance(up, str) and up in packs and up not in seen else {}
        return merged(base, packs[name])
    out = []
    for name in packs:
        pack, file = resolved(name), f"packs/{name}.json"
        for species, rule in (pack.get("trees") or {}).items():
            model = rule and rule.get("model", species)
            if rule and model not in trees and not (rule.get("cover") == "shrub" and model in plants):
                out.append(f"{file}: trees.{species}: the engine has no model {model}")
        for species, rule in (pack.get("ground") or {}).items():
            if rule and rule.get("model", species) not in plants:
                out.append(f"{file}: ground.{species}: the engine has no plant {rule.get('model', species)}")
        if isinstance(pack.get("orchard"), str) and pack["orchard"] not in trees:
            out.append(f"{file}: orchard: the engine has no tree {pack['orchard']}")
    return out


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
    """Entries keyed by species, where null drops the parent's."""
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


def tables(materials):
    """The key tables of the region, pack and style files, which name the materials of `materials`."""
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
        "moor": obj({"heather": colour, "share": number(0, 1)}),
        "speckle": obj({"colour": colour, "share": number(0, 1), "size_m": number(0, 50)}),
        "materials": each(materials["materials"], obj({"colour": paint, "satellite": number(0, 1), "stones": number(0, 1)})),
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
        "cliffs": obj({"reach_m": number(0, 1000), "raise_m": number(0, 500), "min_m": number(0, 1000)}),
        "summits": obj({"reach_m": number(0, 5000), "max_cut_m": number(0, 1000)}),
        "relief": obj({"radius_m": number(0, 1000), "amount": number(0, 5)}),
        "look": look,
    })
    species = obj({
        "model": text, "latin": text, "leaf": one_of("needleleaved", "broadleaved"), "cover": one_of("tree", "shrub"),
        "elevation": listed(number(-500, 9000), 2), "weight": positive, "evergreen": one_of(True, False),
    })
    ground = obj({
        "model": text, "latin": text, "on": listed(one_of(*COVERS)), "height": listed(number(0, 50), 2), "share": number(0, 1),
        "habitat": one_of("moor", "lush"), "own": one_of(True), "clumped": one_of(True),
    })
    herd = obj({"model": text, "latin": text, "groups": number(1, 100), "size": listed(number(1, 1000), 2),
                "on": listed(one_of(*materials["materials"]))})
    pack = obj({
        "name": text, "about": text, "extends": text, "trees": entries(species), "ground": entries(ground),
        "herds": entries(herd), "style": by_place(text), "houses": text, "orchard": text,
    })
    style = obj({
        "name": text, "about": text, "extends": text, "pitch": listed(number(0, 80), 2), "eave": number(0, 5), "verge": number(0, 5),
        "hip": number(0, 1), "timber": one_of("none", "upper", "all", "frame"), "shutters": one_of(True, False),
        "balconies": one_of(True, False), "chimneys": one_of(0, 1, 2), "spire": one_of("onion"), "storeys": number(1, 8),
        "palette": obj({"render": listed(colour), "wood": listed(colour), "roof": listed(colour), "door": listed(colour),
                        "shutter": listed(colour), "plinth": colour, "chimney": colour}),
    })
    return region, pack, style, obj({"look": obj(OVERLAY_LOOK)})


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
    """The ruleset: the vocabulary, the ecoregions and each file of the folders by name."""
    folder = lambda name: {path.stem: read(root, path.relative_to(root).as_posix()) for path in sorted((root / name).glob("*.json"))}
    vocabulary = read(root, "materials.json")
    return {
        "materials": vocabulary, "ecoregions": read(root, "ecoregions.json")["regions"],
        "regions": folder("regions"), "packs": folder("packs"), "styles": folder("styles"), "overlays": folder("overlays"),
    }


def vocabulary_problems(vocabulary):
    out = []
    for i, entry in enumerate(vocabulary["materials"]):
        if entry["cover"] not in COVER_ORDER:
            out.append(f"materials.json: materials[{i}].cover: {entry['cover']} is not one of {', '.join(COVER_ORDER)}")
    for key in ("materials", "lithologies"):
        ids = [entry["id"] for entry in vocabulary[key]]
        out += [f"materials.json: {key}[{i}].id: {id_} is also {key}[{ids.index(id_)}]" for i, id_ in enumerate(ids) if ids.index(id_) != i]
    return out


def validate(ruleset):
    """Every problem of the ruleset, one line each: the file, the key path and what is wrong."""
    vocabulary = ruleset["materials"]
    out = vocabulary_problems(vocabulary)
    if len(ruleset["regions"]) > MAX_REGIONS:
        out.append(f"regions/: {len(ruleset['regions'])} regions, more than the {MAX_REGIONS} that one byte of the region tiles holds")
    ids = {key: [entry["id"] for entry in vocabulary[key]] for key in ("materials", "lithologies")}
    region_check, pack_check, style_check, overlay_check = tables(ids)
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

    for name, overlay in ruleset["overlays"].items():
        file = f"overlays/{name}.json"
        add(file, overlay_check(overlay, "") if name in regions else [f"has no regions/{name}.json"])
    return out + model_problems(packs, *models())


def bundle(ruleset):
    """`rules.json` and `regions.json`: a region's id is its index in the sorted list of regions."""
    names = sorted(ruleset["regions"])
    plain = lambda data: {key: value for key, value in data.items() if key not in ("about", "where")}
    rules = {
        "format": FORMAT, "materials": ruleset["materials"],
        "regions": [{"id": name, **plain(ruleset["regions"][name])} for name in names],
        "packs": {name: plain(data) for name, data in ruleset["packs"].items()},
        "styles": {name: plain(data) for name, data in ruleset["styles"].items()},
        "overlays": ruleset["overlays"],
    }
    regions = {
        "format": FORMAT,
        "regions": [{"id": name, **{key: data[key] for key in ("extends", "where") if key in data}}
                    for name, data in ((name, ruleset["regions"][name]) for name in names)],
        "ecoregions": [{"short": e["short"], "polygons": e["polygons"]} for e in ruleset["ecoregions"] if e.get("short") in ruleset["regions"]],
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
    for name, value in (("rules.json", rules), ("regions.json", regions), ("materials.json", ruleset["materials"])):
        (output / name).write_bytes(encoded(value))
    step_request.metrics(request, {"regions": len(rules["regions"]), "rules_bytes": (output / "rules.json").stat().st_size})


def main():
    problems = validate(load())
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        sys.exit(f"{len(problems)} problems")
    print(f"data/trailview: no problems, ruleset {digest()}")


if __name__ == "__main__":
    step() if sys.argv[1:] == ["--step"] else main()
