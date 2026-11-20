"""The routes of the route sources in a box: `route-sources.json`, as `specs/obc-data.md` defines it.

A description `data/routes/<id>.toml` says which layers and fields of the files of a source make a
route and its stages, so a new source needs no code."""

import argparse
import hashlib
import json
import math
import re
import string
import tomllib
from pathlib import Path

from . import step_request

FORMAT = 1
KINDS = ("hiking", "bicycle", "mtb")
TEMPLATES = ("ref", "name", "description", "operator", "website")
SHARED = {"layer", "file", "ascent", "keep", "drop", *TEMPLATES}
KEYS = {
    "description": {"origin", "countries", "kind", "route", "stage"},
    "route": SHARED | {"key", "rank"},
    "stage": SHARED | {"of", "number"},
}
NEEDS = {"description": {"origin", "countries", "kind", "route"}, "route": {"key"}, "stage": {"of", "number"}}
KEY_LIMIT = 10**10
# One metre in degrees of latitude: the tolerance of the simplification.
METRE = 1 / 111_320


def parse(text):
    """The description in the TOML `text`. A ValueError names a key that is unknown, missing or wrong."""
    description = tomllib.loads(text)
    tables = {"description": description, **{name: description[name] for name in ("route", "stage") if name in description}}
    for name, table in tables.items():
        if not isinstance(table, dict):
            raise ValueError(f"`{name}` is a table")
        if unknown := set(table) - KEYS[name]:
            raise ValueError(f"`{name}` has the unknown keys {', '.join(sorted(unknown))}")
        if missing := NEEDS[name] - set(table):
            raise ValueError(f"`{name}` needs {', '.join(sorted(missing))}")
    origin, countries, kind = description["origin"], description["countries"], description["kind"]
    if type(origin) is not int or not 1 <= origin <= 9006:
        raise ValueError("`origin` is an integer from 1 to 9006")
    if not countries or not all(isinstance(code, str) and re.fullmatch("[A-Z]{2}", code) for code in countries):
        raise ValueError("`countries` lists ISO 3166-1 alpha-2 codes, at least one")
    kinds = [kind] if isinstance(kind, str) else values_of(kind) if isinstance(kind, dict) else None
    if not kinds or any(value not in KINDS for value in kinds):
        raise ValueError(f"`kind` is one of {', '.join(KINDS)}, or a table of `field` and `values` that map to them")
    if "rank" in description["route"] and any(type(level) is not int or not 0 <= level <= 4
                                               for level in values_of(description["route"]["rank"]) or [None]):
        raise ValueError("`rank` is a table of `field` and `values` that map to the levels 0 to 4")
    for name, table in tables.items():
        names = {key: table.get(key) for key in ("layer", "file", "of", "number", "ascent", *TEMPLATES)}
        names |= {"key": table.get("key"), "rank.field": (table.get("rank") or {}).get("field")}
        for key, value in names.items():
            if value is not None and not (texts(value) if key == "key" else texts([value])):
                raise ValueError(f"`{name}.{key}` is a {'text or a list of texts' if key == 'key' else 'text'}")
        for key in ("keep", "drop"):
            if key in table and not (isinstance(table[key], dict) and all(isinstance(v, list) for v in table[key].values())):
                raise ValueError(f"`{name}.{key}` maps each field to a list of values")
    return description


def texts(value):
    """Whether `value` is a non-empty text, or a non-empty list of them."""
    value = value if isinstance(value, list) else [value]
    return bool(value) and all(isinstance(item, str) and item for item in value)


def values_of(table):
    """The values of a table `{ field, values }`, or None when it is not one."""
    ok = isinstance(table, dict) and set(table) == {"field", "values"} and isinstance(table["values"], dict)
    return list(table["values"].values()) if ok else None


def text_key(text):
    """The key of a route whose key field holds a text."""
    return int(hashlib.sha256(text.encode()).hexdigest(), 16) % KEY_LIMIT


def key_of(text, seen, where):
    """The key of the field value `text`; `seen` maps each key to the text that gave it."""
    number = re.fullmatch("0|[1-9][0-9]{0,9}", text) is not None
    key = int(text) if number else text_key(text)
    if seen.setdefault(key, text) != text:
        raise ValueError(f"{where}: the texts {seen[key]!r} and {text!r} give the one key {key}")
    return key


def listed(value):
    return value if isinstance(value, list) else [value]


def template(key):
    """Whether the route key `key` is a template of field values, such as `"{A}: {B}"`, and not fields."""
    return isinstance(key, str) and "{" in key


def named(text):
    """The fields of the template `text`, but `{route}` and `{key}`."""
    return {field for _, field, _, _ in string.Formatter().parse(text) if field and field not in ("route", "key")}


def fields(table, kind=None):
    """The fields of the file that `table` reads."""
    names = {field for key in TEMPLATES if key in table for field in named(table[key])}
    names |= set(table.get("keep", {})) | set(table.get("drop", {}))
    names |= {value for key in ("ascent", "of", "number") if (value := table.get(key))}
    key = table.get("key", [])
    names |= named(key) if template(key) else set(listed(key))
    names |= {table[key]["field"] for key in ("rank",) if key in table}
    return names | ({kind["field"]} if isinstance(kind, dict) else set())


def passes(row, table):
    """Whether `row` has a value of each field of `keep` and none of `drop`."""
    text = lambda field: None if row[field] is None else str(row[field])
    return (all(text(field) in map(str, values) for field, values in table.get("keep", {}).items())
            and not any(text(field) in map(str, values) for field, values in table.get("drop", {}).items()))


def lines(wkb, to_wgs84):
    """Each line of the geometry `wkb` in microdegrees, an array of points, simplified within 1 m and
    without repeated points. Only a line or a part of a multi-line or a collection is one: a start
    point or the ring of a polygon is not. A line of fewer than two points is left out."""
    import numpy as np
    import shapely

    # Two levels: a collection can hold a multi-line.
    parts = [] if wkb is None else shapely.get_parts(shapely.get_parts(shapely.from_wkb(wkb)))
    if len(parts):
        line = np.isin(shapely.get_type_id(parts), [shapely.GeometryType.LINESTRING, shapely.GeometryType.LINEARRING])
        parts = parts[line & (shapely.get_num_coordinates(parts) >= 2)]
    if not len(parts):
        return []
    # All parts of a feature at once: a route can have thousands of them.
    xy, index = shapely.get_coordinates(parts, return_index=True)
    lon, lat = to_wgs84.transform(xy[:, 0], xy[:, 1])
    # Scaled so that a degree east is as long as a degree north, at the middle of each line.
    scale = np.cos(np.radians(np.bincount(index, lat) / np.bincount(index)))
    simple = shapely.simplify(shapely.linestrings(lon * scale[index], lat, indices=index), METRE, preserve_topology=False)
    xy, index = shapely.get_coordinates(simple, return_index=True)
    micro = np.rint(np.column_stack([xy[:, 0] / scale[index], xy[:, 1]]) * 1e6).astype(np.int64)
    first = np.r_[True, index[1:] != index[:-1]]
    keep = first | np.r_[True, (np.diff(micro, axis=0) != 0).any(axis=1)]
    micro, index = micro[keep], index[keep]
    split = np.split(micro, np.flatnonzero(np.r_[False, index[1:] != index[:-1]]))
    return [line for line in split if len(line) >= 2]


def features(name, files, table, wanted):
    """`(fid, row, lines)` of each feature of the layer of `table` that `passes`, in the order of the
    feature ids. `row` holds the fields `wanted`."""
    import numpy as np
    import pyogrio
    import pyproj

    if len(files) != 1:
        raise ValueError(f"{name}: a route source has one file, not {len(files)}")
    path = str(next(iter(files.values())))
    # The braces mark the zip: a store object has no `.zip` that would end it.
    path = f"/vsizip/{{{path}}}/{table['file']}" if "file" in table else path
    layers = [row[0] for row in pyogrio.list_layers(path)]
    # A GeoJSON without a `name` has the name of its file as its layer, and a store object is named
    # by its digest: a file of one layer needs no `layer`.
    layer = table.get("layer", layers[0] if len(layers) == 1 else None)
    if layer not in layers:
        raise ValueError(f"{name}: the file has no layer {layer}" if "layer" in table
                         else f"{name}: the file has {len(layers)} layers; name one with `layer`")
    if missing := wanted - set(pyogrio.read_info(path, layer=layer)["fields"]):
        raise ValueError(f"{name}: the layer {layer} has no field {', '.join(sorted(missing))}")
    meta, fids, geometry, values = pyogrio.raw.read(path, layer=layer, columns=sorted(wanted), force_2d=True, return_fids=True)
    to_wgs84 = pyproj.Transformer.from_crs(meta["crs"], "EPSG:4326", always_xy=True)
    # pyogrio reads an integer field with a null as floats: a whole float is the integer again.
    plain = lambda value: (None if math.isnan(value) else int(value) if value.is_integer() else value) if isinstance(value, float) else value
    out = []
    for i in np.argsort(fids, kind="stable"):
        row = {field: plain(column[i].item() if hasattr(column[i], "item") else column[i]) for field, column in zip(meta["fields"], values)}
        if passes(row, table):
            out.append((int(fids[i]), row, lines(geometry[i], to_wgs84)))
    return out


def filled(table, row, extra):
    """The templates of `table` filled from `row`, with outer spaces removed; an empty one is absent."""
    values = {field: "" if value is None else str(value) for field, value in row.items()} | extra
    out = {key: table[key].format_map(values).strip() for key in TEMPLATES if key in table}
    if (field := table.get("ascent")) and row[field] not in (None, ""):
        out["ascent_m"] = round(float(row[field]))
    return {key: value for key, value in out.items() if value != ""}


def touches(members, box):
    west, south, east, north = box
    return any(((line[:, 0] >= west) & (line[:, 0] <= east) & (line[:, 1] >= south) & (line[:, 1] <= north)).any()
               for line in members)


def flat(members):
    return [line.ravel().tolist() for line in members]


def read(source, files, bounds):
    """The entry of `source` ({id, credit, description}) in `route-sources.json`: the routes of its
    `files` ({name: path}) with a point in `bounds` (west, south, east, north)."""
    name = source["id"]
    description = parse(source["description"])
    route, stage, kind = description["route"], description.get("stage"), description["kind"]
    box = [round(value * 1e6) for value in bounds]
    seen = {}

    def keys(row, key, where):
        """`(key, text)` of each field of `key` that has a value in `row`, or of the template `key`
        filled when each of its fields has one."""
        if template(key):
            values = {field: row[field] for field in named(key)}
            text = "" if "" in values.values() or None in values.values() else key.format_map(values).strip()
            texts = [text] if text else []
        else:
            texts = [str(row[field]) for field in listed(key) if row[field] not in (None, "")]
        return [(key_of(text, seen, where), text) for text in texts]

    routes = {}
    for fid, row, found in features(name, files, route, fields(route, kind)):
        kind_of = kind if isinstance(kind, str) else kind["values"].get(str(row[kind["field"]]))
        if kind_of is None:
            continue
        for key, text in keys(row, route["key"], f"{name}: {route.get('layer', 'route')} feature {fid}"):
            routes.setdefault(key, {"row": row, "text": text, "kind": kind_of, "members": []})["members"] += found
    stages = {}
    for fid, row, found in features(name, files, stage, fields(stage)) if stage else []:
        where = f"{name}: {stage.get('layer', 'stage')} feature {fid}"
        for key, _ in keys(row, stage["of"], where):
            if key in routes:
                stages.setdefault(key, []).append((where, row, found))

    out = []
    for key in sorted(routes):
        entry = routes[key]
        rank = route["rank"]["values"].get(str(entry["row"][route["rank"]["field"]]), 0) if "rank" in route else 0
        record = {"key": key, "kind": entry["kind"], "rank": rank, **filled(route, entry["row"], {"key": entry["text"]})}
        parts = stages.get(key, [])
        if len(parts) >= 2:
            numbers = {}
            for where, row, found in parts:
                number = str(row[stage["number"]])
                if not re.fullmatch("[1-9][0-9]?", number) or number in numbers:
                    raise ValueError(f"{where}: the stage number {number} is not an integer from 1 to 99 that is unique in its route")
                numbers[number] = {"stage": int(number), "members": flat(found) if touches(found, box) else [],
                                   **filled(stage, row, {"route": record.get("name", ""), "key": entry["text"]})}
            if any(part["members"] for part in numbers.values()):
                out.append(record | {"stages": sorted(numbers.values(), key=lambda part: part["stage"])})
        elif touches(entry["members"], box):
            out.append(record | {"members": flat(entry["members"])})
    return {"credit": source["credit"], "id": name, "origin": description["origin"], "routes": out}


def write(sources, output):
    """Write `route-sources.json` of `sources` (entries of `read`) into the folder `output`, and return its bytes."""
    text = json.dumps({"format": FORMAT, "sources": sources}, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
    path = Path(output) / "route-sources.json"
    path.write_text(text, encoding="utf-8")
    return path.stat().st_size


def step():
    request = step_request.read()
    options = request["options"]
    sources = [read(source, step_request.files(request, source["id"]), options["bounds"]) for source in options["sources"]]
    size = write(sources, request["output"])
    step_request.metrics(request, {"bytes": size, "routes": {source["id"]: len(source["routes"]) for source in sources}})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--step", action="store_true", required=True)
    parser.parse_args()
    step()


if __name__ == "__main__":
    main()
