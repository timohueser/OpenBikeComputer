"""Validate and canonicalize the finite planner query language and model labels.

`python3 schema.py` writes contract.json, the language as data for the JS, TS and Swift clients."""

from __future__ import annotations

import json
import unicodedata
from pathlib import Path
from typing import Any

# The model's intent head in output order; a change needs a retrained model. `reroute` has no
# request type, so the decoder returns `none` for it.
INTENTS = [
    "places", "place", "route", "stretches", "end_day", "add_point", "remove_point",
    "split", "join", "reverse", "reroute", "none",
]

SLOTS = [
    "WHAT",     # place kinds or a stretch kind
    "NAME",     # the name of a `place` request
    "FROM", "TO", "VIA",           # route points
    "NEAR",     # near a point; two NEAR spans mean "between"
    "POINT",    # the point of end_day, add_point, remove_point
    "BEFORE", "AFTER",             # on the plan before / after a point
    "DAY",      # day and part of day: "end of day 4", "tomorrow", "every night"
    "SCOPE",    # "along the route", "in the map view", "around here"
    "ALONG",    # km or time on the plan: "km 40 to 60", "first 20 km", "after 3 hours"
    "OPEN",     # "open on Sunday", "open now", "open"
    "RADIUS",   # "within 3 km"
    "BIKE", "GOAL",
    "DAYS",     # a day count: "in 7 days", "into 5 days"
    "PER_DAY",  # "80 km a day", "5 hours per day"
    "EVERY",    # "every 3 hours"
    "MIN",      # a threshold: "longer than 30 km", "than 10 %"
    "KIND",     # point kind: "as a stop", "pass through"
    "IGNORED",
]

LABELS = ["O"] + [f"{p}-{s}" for s in SLOTS for p in ("B", "I")]

# Place kinds for planning. Not tied to any map format; the resolver maps them to its data.
# id -> parent category (None for a category).
KINDS: dict[str, str | None] = {
    "water": None, "drinking_water": "water", "fountain": "water", "spring": "water",
    "water_tap": "water",
    "sleep": None,
    "campsite": "sleep", "shelter": "sleep",
    "lodging": "sleep", "hotel": "lodging", "hostel": "lodging", "guest_house": "lodging",
    "motel": "lodging", "hut": "lodging",
    "resupply": None, "supermarket": "resupply", "convenience": "resupply",
    "bakery": "resupply", "butcher": "resupply", "marketplace": "resupply", "fuel": "resupply",
    "food": None, "cafe": "food", "restaurant": "food", "fast_food": "food", "bar": "food",
    "ice_cream": "food",
    "pharmacy": None,
    "medical": None, "hospital": "medical", "doctor": "medical",
    "bike": None, "bike_shop": "bike", "repair_station": "bike", "charging": "bike",
    "toilets": None, "shower": None, "laundry": None, "atm": None,
    "transport": None, "train_station": "transport", "bus_stop": "transport",
    "ferry": "transport",
    "swimming": None, "lake": "swimming", "beach": "swimming", "swimming_pool": "swimming",
    "sight": None, "viewpoint": "sight", "castle": "sight", "church": "sight",
    "monastery": "sight", "museum": "sight", "ruins": "sight", "waterfall": "sight",
    "pass": "sight", "summit": "sight", "tower": "sight", "bridge": "sight",
    "town": None,
}

# The map category (builder/app/src/lib/planner/poi-kinds.json) whose icon and layer show a
# kind. A kind without an entry takes its parent's; a kind that gets none shows on no layer.
CATEGORIES = {
    "water": "water", "sleep": "hotel", "campsite": "camp", "shelter": "shelter",
    "resupply": "shop", "food": "food", "pharmacy": "pharmacy", "medical": "pharmacy",
    "bike": "bike", "toilets": "toilets", "shower": "toilets", "laundry": "toilets",
    "transport": "station", "viewpoint": "viewpoint", "summit": "peak", "pass": "peak",
}

# Search-data kinds (records.category) of a kind other than its own id. A parent kind finds the
# data kinds of its children.
DATA_KINDS = {
    "campsite": ["campsite", "caravan_site"], "lodging": ["chalet"],
    "drinking_water": ["drinking_water", "water_point"], "doctor": ["doctor", "clinic"],
    "bar": ["bar", "pub"], "town": ["city", "town", "village", "hamlet"],
}

# Kinds that only a client sends: places of the named kind that serve this cuisine.
CUISINES = {"pizza": "food", "kebab": "food"}

STRETCH_KINDS = ["climb", "descent", "steep", "unpaved", "unknown_surface", "pushing", "closure"]

BIKES = ["road", "gravel", "mtb", "touring"]
GOALS = ["balanced", "shortest", "least_climbing"]
PARTS = ["start", "middle", "end"]
WEEKDAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
POINT_KINDS = ["visit", "stop", "pass"]
SCOPES = ["route", "view", "here"]
REFS = ["km", "start", "end", "here"]
# The units of each quantity field.
UNITS = {"along": ["km", "h"], "radius": ["km"], "every": ["km", "h"], "per_day": ["km", "h"],
         "min": ["km", "m", "%"]}

# Words stripped from the front of a name before two names are compared.
ARTICLES = {
    "the", "a", "an", "der", "die", "das", "den", "dem", "des", "ein", "eine", "einen",
    "le", "la", "les", "l'", "un", "une", "il", "lo", "gli", "i", "uno", "una",
}

# Types that act on the plan: a where of {"scope": "route"} says nothing there.
PLAN_TYPES = {"stretches", "end_day", "add_point", "remove_point", "split", "join", "reverse"}

FIELDS = {
    "places": ({"what"}, {"where", "open", "radius"}),
    "place": ({"name"}, {"near"}),
    "route": ({"to"}, {"from", "via", "bike", "goal", "days", "per_day"}),
    "stretches": ({"what"}, {"where", "min"}),
    "end_day": ({"day", "at"}, set()),
    "add_point": ({"point"}, {"kind", "every", "where"}),
    "remove_point": ({"point"}, set()),
    "split": (set(), {"days", "per_day", "where"}),
    "join": ({"day"}, set()),
    "reverse": (set(), set()),
    "none": (set(), set()),
}


class Invalid(ValueError):
    pass


def _need(ok: bool, msg: str) -> None:
    if not ok:
        raise Invalid(msg)


def _quantity(q: Any, units: list[str]) -> None:
    _need(isinstance(q, dict) and set(q) == {"value", "unit"}, f"quantity {q!r}")
    _need(isinstance(q["value"], (int, float)) and q["value"] >= 0, f"quantity value {q!r}")
    _need(q["unit"] in units, f"unit {q['unit']!r} not in {sorted(units)}")


def _day(d: Any) -> None:
    _need(d in ("today", "tomorrow", "every") or (isinstance(d, int) and d >= 1), f"day {d!r}")


def _along(a: Any) -> None:
    _need(isinstance(a, dict) and a.get("ref") in REFS, f"along {a!r}")
    _need(set(a) <= {"ref", "from", "to", "at"}, f"along keys {a!r}")
    _need(("at" in a) != ("from" in a or "to" in a), f"along needs at, or from/to: {a!r}")
    units = {a[k]["unit"] for k in ("from", "to", "at") if k in a}
    for k in ("from", "to", "at"):
        if k in a:
            _quantity(a[k], UNITS["along"])
    _need(len(units) == 1, f"along mixes units {a!r}")


def _point(p: Any) -> None:
    _need(isinstance(p, dict) and len(p) >= 1, f"point {p!r}")
    if "name" in p:
        _need(set(p) == {"name"} and isinstance(p["name"], str) and p["name"].strip(),
              f"name point {p!r}")
    elif "kind" in p:
        _need(set(p) == {"kind"} and p["kind"] in KINDS, f"kind point {p!r}")
    elif "here" in p:
        _need(p == {"here": True}, f"here point {p!r}")
    elif "plan" in p:
        _need(set(p) == {"plan"} and p["plan"] in ("start", "end"), f"plan point {p!r}")
    elif "day" in p:
        _need(set(p) <= {"day", "part"}, f"day point {p!r}")
        _day(p["day"])
        _need(p.get("part", "end") in PARTS, f"day point part {p!r}")
    elif "along" in p:
        _need(set(p) == {"along"}, f"along point {p!r}")
        _along(p["along"])
        _need("at" in p["along"], f"along point needs a mark {p!r}")
    else:
        raise Invalid(f"point {p!r}")


def _where(w: Any) -> None:
    _need(isinstance(w, dict) and w, f"where {w!r}")
    _need(set(w) <= {"scope", "day", "part", "near", "along", "before", "after"},
          f"where keys {w!r}")
    if "scope" in w:
        _need(w["scope"] in SCOPES, f"scope {w!r}")
    if "day" in w:
        _day(w["day"])
    if "part" in w:
        _need(w["part"] in PARTS and ("day" in w or w.get("scope") == "route"),
              f"part needs a day or route {w!r}")
    if "near" in w:
        _need(isinstance(w["near"], list) and 1 <= len(w["near"]) <= 2, f"near {w!r}")
        for p in w["near"]:
            _point(p)
            _need("kind" not in p, f"near a kind is Level 3 {w!r}")
    if "along" in w:
        _along(w["along"])
    for k in ("before", "after"):
        if k in w:
            _point(w[k])


def validate(r: Any) -> None:
    """Raises Invalid when `r` is not a request of the language."""
    _need(isinstance(r, dict) and r.get("type") in FIELDS, f"type of {r!r}")
    required, optional = FIELDS[r["type"]]
    keys = set(r) - {"type", "ignored"}
    _need(required <= keys, f"{r['type']} needs {sorted(required - keys)}")
    _need(keys <= required | optional, f"{r['type']} does not take {sorted(keys - required - optional)}")
    ign = r.get("ignored", [])
    _need(isinstance(ign, list) and all(isinstance(s, str) and s.strip() for s in ign),
          f"ignored {ign!r}")
    t = r["type"]
    if "what" in r and t == "places":
        _need(isinstance(r["what"], list) and 1 <= len(r["what"]) <= 3, f"what {r['what']!r}")
        for k in r["what"]:
            _need(k in KINDS, f"kind {k!r}")
    if t == "stretches":
        s = r["what"]
        _need(s in STRETCH_KINDS or (isinstance(s, str) and s.startswith("gap:")
                                     and s[4:] in KINDS), f"stretch {s!r}")
    if "where" in r:
        _where(r["where"])
    for k in ("to", "from", "at", "point", "near"):
        if k in r:
            _point(r[k])
    if "via" in r:
        _need(isinstance(r["via"], list) and 1 <= len(r["via"]) <= 2, f"via {r['via']!r}")
        for p in r["via"]:
            _point(p)
    if "name" in r:
        _need(isinstance(r["name"], str) and r["name"].strip(), f"name {r['name']!r}")
    if "open" in r:
        o = r["open"]
        _need(isinstance(o, dict) and len(o) == 1, f"open {o!r}")
        if "weekday" in o:
            _need(o["weekday"] in WEEKDAYS, f"open {o!r}")
        elif "day" in o:
            _day(o["day"])
        else:
            _need(o == {"now": True}, f"open {o!r}")
    if "radius" in r:
        _quantity(r["radius"], UNITS["radius"])
    if "bike" in r:
        _need(r["bike"] in BIKES, f"bike {r['bike']!r}")
    if "goal" in r:
        _need(r["goal"] in GOALS, f"goal {r['goal']!r}")
    if "days" in r:
        _need(isinstance(r["days"], int) and r["days"] >= 1, f"days {r['days']!r}")
    if "day" in r:
        if t == "join":
            _need(isinstance(r["day"], int) and r["day"] >= 1, f"join day {r['day']!r}")
        else:
            _day(r["day"])
    if "kind" in r:
        _need(r["kind"] in POINT_KINDS, f"point kind {r['kind']!r}")
    if "every" in r:
        _quantity(r["every"], UNITS["every"])
    if "per_day" in r:
        _quantity(r["per_day"], UNITS["per_day"])
    if "min" in r:
        _quantity(r["min"], UNITS["min"])
    if t == "split":
        _need(("days" in r) != ("per_day" in r), "split needs exactly one of days, per_day")


def fold_name(name: str) -> str:
    """The comparison form of a name: case-folded, spaces collapsed, no leading article."""
    s = unicodedata.normalize("NFC", name).casefold().strip(" \t.,;:!?\"'«»()")
    words = s.split()
    while len(words) > 1 and words[0] in ARTICLES:
        words = words[1:]
    if words and words[0].startswith(("l'", "l’")) and len(words[0]) > 2:
        words[0] = words[0][2:]
    return " ".join(words)


def _canon_point(p: dict) -> dict:
    p = dict(p)
    if "name" in p:
        p["name"] = fold_name(p["name"])
    if "day" in p and "part" not in p:
        p["part"] = "end"
    if "along" in p:
        p["along"] = _canon_along(p["along"])
    return p


def _canon_q(q: dict) -> dict:
    return {"value": round(float(q["value"]), 3), "unit": q["unit"]}


def _canon_along(a: dict) -> dict:
    out = {"ref": a["ref"]}
    for k in ("from", "to", "at"):
        if k in a:
            out[k] = _canon_q(a[k])
    if out.get("from", {}).get("value") == 0:
        del out["from"]
    return out


def _normal_where(w: dict) -> dict:
    w = dict(w)
    if w.get("near") == [{"here": True}] and "scope" not in w:
        del w["near"]
        w["scope"] = "here"
    # "after km 80" and "after 80 km" ask for the same mark.
    if "along" not in w and "along" in w.get("after", {}) and "at" in w["after"]["along"]:
        w["along"] = w.pop("after")["along"]
    # Without a named day, the plan start is km 0.
    if "along" in w and w["along"]["ref"] == "start" and "day" not in w:
        w["along"] = {**w["along"], "ref": "km"}
    # A day, a stretch or a point on the plan already says "on the route".
    if w.get("scope") == "route" and set(w) & {"day", "along", "before", "after"}:
        del w["scope"]
    return w


def normal(r: dict) -> dict:
    """One form for places on the plan that two spellings name. The decoder returns this form, so
    the resolver never sees the other spelling."""
    return {**r, "where": _normal_where(r["where"])} if "where" in r else r


def _canon_where(w: dict) -> dict:
    w = _normal_where(w)
    if "near" in w:
        w["near"] = [_canon_point(p) for p in w["near"]]
    for k in ("before", "after"):
        if k in w:
            w[k] = _canon_point(w[k])
    if "along" in w:
        w["along"] = _canon_along(w["along"])
    return w


def canonical(r: dict) -> dict:
    """The one form two equal requests share. `ignored` becomes a bool: its words may differ."""
    r = dict(r)
    t = r["type"]
    out: dict[str, Any] = {"type": t}
    if t == "none":
        return out
    for k, v in r.items():
        if k in ("type", "ignored"):
            continue
        if k in ("to", "from", "at", "point"):
            v = _canon_point(v)
        elif k == "near" and t == "place":
            v = _canon_point(v)
        elif k == "via":
            v = [_canon_point(p) for p in v]
        elif k == "where":
            v = _canon_where(v)
        elif k == "name":
            v = fold_name(v)
        elif k == "what" and t == "places":
            v = sorted(set(v))
        elif k in ("every", "per_day", "min", "radius"):
            v = _canon_q(v)
        out[k] = v
    if t in PLAN_TYPES and out.get("where") == {"scope": "route"}:
        del out["where"]
    if out.get("goal") == "balanced" and t == "route":
        del out["goal"]
    # Visit or pass follows from the place (a town is passed, a shop is visited); a repeat
    # always adds stops.
    if t == "add_point":
        if "every" in out:
            out["kind"] = "stop"
        elif out.get("kind") != "stop":
            out.pop("kind", None)
    out["ignored"] = bool(r.get("ignored"))
    return out


def same(pred: dict, gold: dict) -> bool:
    return canonical(pred) == canonical(gold)


def _category(kind: str | None) -> str | None:
    return CATEGORIES[kind] if kind in CATEGORIES else kind and _category(KINDS[kind])


def contract() -> dict:
    """The language for clients. `kinds[k].data` holds the search-data kinds of k and its
    children, `data` names the most specific kind of each data kind, and `categories` lists the
    largest kinds whose places all show in one map category."""
    children = {k: [c for c, parent in KINDS.items() if parent == k] for k in KINDS}

    def tree(k: str) -> list[str]:
        return [k, *(d for c in children[k] for d in tree(c))]

    own = {k: DATA_KINDS.get(k, [] if children[k] else [k]) for k in KINDS}
    kinds = {k: {"parent": parent, "category": _category(k),
                 "data": [d for c in tree(k) for d in own[c]]} for k, parent in KINDS.items()}
    within = lambda k, category: k is not None and all(_category(c) == category for c in tree(k))
    categories: dict[str, list[str]] = {}
    for k, parent in KINDS.items():
        category = _category(k)
        if category and within(k, category) and not within(parent, category):
            categories.setdefault(category, []).append(k)
    return {"types": list(FIELDS), "kinds": kinds, "data": {d: k for k in KINDS for d in own[k]},
            "categories": categories, "cuisines": CUISINES, "stretches": STRETCH_KINDS,
            "bikes": BIKES, "goals": GOALS, "parts": PARTS, "weekdays": WEEKDAYS,
            "point_kinds": POINT_KINDS, "scopes": SCOPES, "refs": REFS, "units": UNITS}


CONTRACT = Path(__file__).with_name("contract.json")


def contract_json() -> str:
    return json.dumps(contract(), indent=1) + "\n"


if __name__ == "__main__":
    CONTRACT.write_text(contract_json())
