"""Prototype: how well can the surface class of a road without a `surface` tag be estimated? Writes Markdown tables."""

import re
import sys
import time
from pathlib import Path

import numpy as np
from scipy import sparse
from sklearn.ensemble import HistGradientBoostingClassifier

sys.path.insert(0, str(Path(__file__).parent))  # `python -I` leaves the script's directory out
from extract import KINDS, LAND  # noqa: E402

CLASSES = ["paved", "compacted", "gravel", "dirt", "rough"]
# `surface_class` in planner/router-build/src/osm.rs.
SURFACE = {
    "asphalt": 0, "paved": 0, "concrete": 0, "concrete:plates": 0, "concrete:lanes": 0, "paving_stones": 0,
    "compacted": 1, "fine_gravel": 1,
    "gravel": 2, "pebblestone": 2,
    "ground": 3, "dirt": 3, "earth": 3, "grass": 3, "unpaved": 3,
    "sand": 4, "mud": 4, "rock": 4, "stone": 4, "cobblestone": 4, "sett": 4,
}  # fmt: skip
FREIBURG_BOX = (7.50, 47.78, 8.24, 48.31)
CELL_KM = 2.0
TARGET = 0.95
TRACK = KINDS.index("track")
REPORT_KINDS = [*KINDS, "track, no tracktype"]
REPORT_KINDS[TRACK] = "track, with tracktype"
SPEEDS = {"walk": 7, "none": 130, "de:urban": 50, "de:rural": 100, "fr:urban": 50, "fr:rural": 80, "gb:nsl_single": 97}
SPECS = {
    "b own tags": ["own"],
    "c + neighbours": ["own", "neighbour kinds", "strict"],
    "c-open": ["own", "neighbour kinds", "open"],
    "d + land use": ["own", "neighbour kinds", "strict", "land"],
}
MAIN = "c + neighbours"


def coded(values, function):
    unique, inverse = np.unique(values, return_inverse=True)
    return np.array([function(value) for value in unique], np.float32)[inverse]


def vocabulary(*words):
    table = {word: index + 1 for index, word in enumerate(words)}
    return lambda value: table.get(value, len(words) + 1) if value else 0


def ordered(*words):
    table = {word[:24]: index + 1 for index, word in enumerate(words)}
    return lambda value: table.get(value, np.nan)


def number(value):
    match = re.match(r"\d+(\.\d+)?", value.replace(",", "."))
    return float(match.group()) if match else np.nan


def speed(value):
    value = value.lower()
    return SPEEDS.get(value, number(value) * (1.609 if "mph" in value else 1))


ACCESS = vocabulary(
    "yes", "no", "designated", "permissive", "private", "destination", "dismount", "use_sidepath", "agricultural",
    "forestry", "customers", "delivery", "official",
)  # fmt: skip
YES_NO = vocabulary("yes", "no")
GRADE = ordered("grade1", "grade2", "grade3", "grade4", "grade5")


def load(path, holdout, outside_freiburg=False):
    d = dict(np.load(path))
    d["n"] = len(d["id"])
    values, inverse = np.unique(d["tag_surface"], return_inverse=True)
    d["y"] = np.array([SURFACE.get(value, -1) for value in values])[inverse]
    d["km"] = d["length"] / 1000
    d["cx"] = np.floor(d["lon"] * 111.32 * np.cos(np.radians(d["lat"].mean())) / CELL_KM).astype(np.int64)
    d["cy"] = np.floor(d["lat"] * 111.32 / CELL_KM).astype(np.int64)
    cells, inverse = np.unique(d["cx"] * 2**20 + d["cy"], return_inverse=True)
    d["fold"] = (np.random.default_rng(0).permutation(len(cells)) % 5)[inverse]
    # With a hold-out, fold 0 is the test set and its ways are never neighbours.
    d["holdout"] = holdout
    d["grade"] = np.nan_to_num(coded(d["tag_tracktype"], GRADE)).astype(int)
    d["rk"] = np.where((d["kind"] == TRACK) & (d["grade"] == 0), len(KINDS), d["kind"])
    west, south, east, north = FREIBURG_BOX
    inside = (d["lon"] >= west) & (d["lon"] <= east) & (d["lat"] >= south) & (d["lat"] <= north)
    d["use"] = ~inside if outside_freiburg else np.ones(d["n"], bool)
    return d


def own_block(d):
    def tag(name):
        return d["tag_" + name]

    columns = [
        ("kind", "kind", True, d["kind"]),
        ("tracktype", "tracktype", False, coded(tag("tracktype"), GRADE)),
        ("smoothness", "smoothness", False, coded(tag("smoothness"), ordered(
            "excellent", "good", "intermediate", "bad", "very_bad", "horrible", "very_horrible", "impassable"))),
        ("bicycle", "access tags", True, coded(tag("bicycle"), ACCESS)),
        ("foot", "access tags", True, coded(tag("foot"), ACCESS)),
        ("access", "access tags", True, coded(tag("access"), ACCESS)),
        ("motor_vehicle", "access tags", True, coded(tag("motor_vehicle"), ACCESS)),
        ("segregated", "access tags", True, coded(tag("segregated"), YES_NO)),
        ("lit", "road tags", True, coded(tag("lit"), YES_NO)),
        ("maxspeed", "road tags", False, coded(tag("maxspeed"), speed)),
        ("width", "road tags", False, coded(tag("width"), number)),
        ("service", "road tags", True, coded(tag("service"), vocabulary(
            "driveway", "parking_aisle", "alley", "emergency_access", "drive-through"))),
        ("named", "road tags", False, d["name"] > 0),
        ("bridge", "road tags", False, coded(tag("bridge"), lambda value: value not in ("", "no"))),
        ("tunnel", "road tags", False, coded(tag("tunnel"), lambda value: value not in ("", "no"))),
        ("mtb:scale", "trail scales", False, coded(tag("mtb:scale"), number)),
        ("sac_scale", "trail scales", False, coded(tag("sac_scale"), ordered(
            "hiking", "mountain_hiking", "demanding_mountain_hiking", "alpine_hiking", "demanding_alpine_hiking",
            "difficult_alpine_hiking"))),
        ("route=bicycle level", "route relations", False, d["routes"][:, 0]),
        ("route=mtb", "route relations", False, d["routes"][:, 1]),
        ("route=hiking|foot level", "route relations", False, d["routes"][:, 2]),
        # Not in the brief's tag list.
        ("footway", "extra tags", True, coded(tag("footway"), vocabulary(
            "sidewalk", "crossing", "access_aisle", "traffic_island", "link"))),
        ("lanes", "extra tags", False, coded(tag("lanes"), number)),
        ("oneway", "extra tags", True, coded(tag("oneway"), YES_NO)),
        ("trail_visibility", "extra tags", False, coded(tag("trail_visibility"), ordered(
            "excellent", "good", "intermediate", "bad", "horrible", "no"))),
        ("ref", "extra tags", False, tag("ref") != ""),
        ("log length", "way length", False, np.log10(d["length"].clip(1))),
    ]  # fmt: skip
    return block(columns)


def block(columns):
    names, groups, categorical, values = zip(*columns)
    return np.column_stack([np.asarray(value, np.float32) for value in values]), names, groups, categorical


def links(query_rows, query_keys, source_rows, source_keys, n):
    """`query @ (source.T @ Y)` sums Y over the ways that share a key with each way; `own` counts the keys it shares
    with itself."""
    _, inverse = np.unique(np.concatenate([query_keys, source_keys]), return_inverse=True)

    def incidence(rows, columns):
        matrix = sparse.csr_matrix((np.ones(len(rows), np.float64), (rows, columns)), shape=(n, inverse.max() + 1))
        matrix.data[:] = 1
        return matrix

    query = incidence(query_rows, inverse[: len(query_keys)])
    source = incidence(source_rows, inverse[len(query_keys) :])
    return query, source, np.asarray(query.multiply(source).sum(axis=1)).ravel()


def link_sets(d):
    n, kind = d["n"], d["kind"].astype(np.int64)
    rows = np.arange(n)
    end_rows, ends = np.concatenate([rows, rows]), np.concatenate([d["first"], d["last"]])

    def squares(group, mask):
        """Ways of one group in the 3 x 3 squares around the way."""
        index = np.flatnonzero(mask)

        def key(dx, dy):
            return (group[index] << 40) | ((d["cx"][index] + dx + 2**19) << 20) | (d["cy"][index] + dy + 2**19)

        keys = np.concatenate([key(dx, dy) for dx in (-1, 0, 1) for dy in (-1, 0, 1)])
        return links(np.tile(index, 9), keys, index, key(0, 0), n)

    return {
        "end node": links(end_rows, ends, d["node_way"], d["node_ref"], n),
        "end node, same kind": links(
            end_rows, ends * 16 + kind[end_rows], d["node_way"], d["node_ref"] * 16 + kind[d["node_way"]], n
        ),
        "same name": squares(d["name"].astype(np.int64), d["name"] > 0),
        "squares around, same kind": squares(kind, np.ones(n, bool)),
    }


def surface_block(d, sets, open_):
    """The surface classes of the tagged ways around each way. Strict: only ways of other folds count, so a whole
    square is blind. Open: at the end nodes and for the same name every other tagged way counts, also the next piece
    of the same road."""
    n, tagged = d["n"], d["y"] >= 0
    onehot = np.zeros((n, 5))
    onehot[tagged, d["y"][tagged]] = 1
    columns = []
    for name, (query, source, own) in sets.items():
        weight = onehot if name.startswith("end node") else onehot * d["km"][:, None]
        # The mix of the squares around stays strict: without the way itself, "all others are paved" tells a model
        # that this way is the one that is not.
        if open_ and not name.startswith("squares"):
            sums = query @ (source.T @ weight) - own[:, None] * weight
        else:
            sums = np.zeros((n, 5))
            for fold in range(5):
                rows = d["fold"] == fold
                hidden = rows | ((d["fold"] == 0) & d["holdout"])
                sums[rows] = query[rows] @ (source.T @ (weight * ~hidden[:, None]))
        sums[sums < 1e-6] = 0
        total = sums.sum(axis=1)
        group = {"end node, same kind": "end-node surfaces", "end node": "end-node surfaces"}.get(name, name + " surfaces")
        # Shares only: a count of tagged ways depends on how densely a region is tagged, and does not transfer.
        for index, surface in enumerate(CLASSES):
            columns.append((f"{name}: share {surface}", group, False, sums[:, index] / np.where(total > 0, total, np.nan)))
    return block(columns)


def kinds_block(d, sets):
    query, source, own = sets["end node"]
    onehot = np.zeros((d["n"], len(KINDS)))
    onehot[np.arange(d["n"]), d["kind"]] = 1
    counts = query @ (source.T @ onehot) - own[:, None] * onehot
    return block([(f"end node: ways of kind {kind}", "neighbour kinds", False, counts[:, i]) for i, kind in enumerate(KINDS)])


def blocks(d):
    sets = link_sets(d)
    return {
        "own": own_block(d),
        "neighbour kinds": kinds_block(d, sets),
        "strict": surface_block(d, sets, False),
        "open": surface_block(d, sets, True),
        "land": block([("land use", "land use", True, d["land"])]),
    }


def design(all_blocks, spec, wanted=lambda group: True):
    parts = [all_blocks[name] for name in spec]
    x = np.hstack([part[0] for part in parts])
    names, groups, categorical = ([value for part in parts for value in part[index]] for index in (1, 2, 3))
    keep = [wanted(group) for group in groups]
    return x[:, keep], [n for n, k in zip(names, keep) if k], [c for c, k in zip(categorical, keep) if k]


def fit(x, y, categorical):
    return HistGradientBoostingClassifier(max_iter=100, categorical_features=categorical, random_state=0).fit(x, y)


def probabilities(model, x):
    out = np.zeros((len(x), 5))
    out[:, model.classes_] = model.predict_proba(x)
    return out




def mode_table(keys, y, km, size, default):
    """The class with the most length for each key."""
    length = np.zeros((size, 5))
    np.add.at(length, (keys, y), km)
    return np.where(length.sum(axis=1) > 0, length.argmax(axis=1), default)


def cut_for(confidence, right, target):
    """The lowest confidence at which the claims above it are right at the target rate."""
    order = np.argsort(-confidence)
    precision = np.cumsum(right[order]) / np.arange(1, len(order) + 1)
    good = np.flatnonzero(precision >= target)
    return confidence[order][good[-1]] if len(good) else np.inf


def cuts_for(d, train, proba):
    """The confidence cuts from out-of-fold estimates on the training squares."""
    confidence, right = proba.max(axis=1), proba.argmax(axis=1) == d["y"]
    paved_confidence = np.maximum(proba[:, 0], 1 - proba[:, 0])
    paved_right = (proba[:, 0] >= 0.5) == (d["y"] == 0)

    def each(confidence, right):
        masks = [train & (d["rk"] == kind) for kind in range(len(REPORT_KINDS))]
        return np.array([cut_for(confidence[m], right[m], TARGET) if m.sum() >= 50 else np.inf for m in masks])

    return {
        "all": cut_for(confidence[train], right[train], TARGET),
        "kind": each(confidence, right),
        "paved": each(paved_confidence, paved_right),
    }


def seen(surfaces):
    """A tagged way at an end node is visible."""
    return ~np.isnan(surfaces[0][:, surfaces[1].index("end node: share paved")])


# Reports


def table(header, rows):
    lines = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    lines += ["| " + " | ".join(str(cell) for cell in row) + " |" for row in rows]
    return "\n".join(lines) + "\n\n"


def pct(part, whole, digits=1):
    return "–" if whole == 0 else f"{100 * part / whole:.{digits}f}"


def by_kind(d, rows, grades=False):
    yield "all", rows
    for index, name in enumerate(REPORT_KINDS):
        mask = rows & (d["rk"] == index)
        if mask.any():
            yield name, mask
    for grade in range(1, 6 if grades else 0):
        yield f"track, grade{grade}", rows & (d["kind"] == TRACK) & (d["grade"] == grade)


def flags(d, predicted):
    return {
        "right class of five": predicted == d["y"],
        "right paved or not paved": (predicted == 0) == (d["y"] == 0),
        "estimated paved or compacted, truly dirt or rough": (predicted <= 1) & (d["y"] >= 3),
    }


def coverage_report(name, d):
    out = f"## {name}\n\n"
    rows = []
    for label, mask in by_kind(d, d["use"], True):
        km, tagged = d["km"][mask], d["y"][mask] >= 0
        other = (d["tag_surface"][mask] != "") & ~tagged
        mix = [pct(km[d["y"][mask] == index].sum(), km[tagged].sum(), 0) for index in range(5)]
        rows.append([label, mask.sum(), f"{km.sum():.0f}", pct(km[tagged].sum(), km.sum()), pct(tagged.sum(), mask.sum()),
                     pct(km[other].sum(), km.sum()), *mix])  # fmt: skip
    out += "Share with a usable `surface` tag, and the class mix of the tagged length (% of tagged km).\n\n"
    out += table(["road kind", "ways", "km", "usable surface, % of km", "usable surface, % of ways",
                  "other surface value, % of km", *CLASSES], rows)  # fmt: skip
    other = d["use"] & (d["tag_surface"] != "") & (d["y"] < 0)
    values, inverse = np.unique(d["tag_surface"][other], return_inverse=True)
    km = np.bincount(inverse, d["km"][other], len(values))
    out += "The `surface` values that the router does not read, by length.\n\n"
    out += table(["value", "km"], [[values[i], f"{km[i]:.0f}"] for i in np.argsort(-km)[:12]])
    return out


def accuracy_report(d, rows, estimates):
    out = ""
    for metric in flags(d, d["y"]):
        for unit in ("ways", "km"):
            lines = []
            for label, mask in by_kind(d, rows, True):
                weight = np.ones(mask.sum()) if unit == "ways" else d["km"][mask]
                cells = [pct(weight[flags(d, e)[metric][mask]].sum(), weight.sum()) for e in estimates.values()]
                lines.append([label, mask.sum(), f"{d['km'][mask].sum():.0f}", *cells])
            out += f"### {metric[0].upper()}{metric[1:]}, % of {unit}\n\n"
            out += table(["road kind", "ways", "km", *estimates], lines)
    return out


def confusion_report(d, rows, estimates):
    out = ""
    for name, predicted in estimates.items():
        for label, mask in by_kind(d, rows):
            lines = []
            for truth, surface in enumerate(CLASSES):
                cells = []
                for estimate in range(5):
                    cell = mask & (d["y"] == truth) & (predicted == estimate)
                    cells.append(f"{cell.sum()} ({d['km'][cell].sum():.0f})")
                lines.append([f"truly {surface}", *cells])
            out += f"### {name}, {label}: ways (km)\n\n"
            out += table(["", *[f"estimated {surface}" for surface in CLASSES]], lines)
    return out


def claims_report(d, rows, proba, cuts):
    km, paved = d["km"], proba[:, 0]
    five, five_confidence = proba.argmax(axis=1), proba.max(axis=1)
    plans = [
        ("one of five classes, one cut for all road kinds", five, five_confidence, np.full(len(REPORT_KINDS), cuts["all"]), False),
        ("one of five classes, one cut for each road kind", five, five_confidence, cuts["kind"], False),
        ("paved or not paved, one cut for each road kind", np.where(paved >= 0.5, 0, 3), np.maximum(paved, 1 - paved), cuts["paved"], True),
    ]  # fmt: skip
    out = f"Each cut gives {TARGET:.0%} right claims on the training squares of Freiburg (out-of-fold estimates). "
    out += "A cut of `inf` makes no claim. A cut for one road kind with few ways is not stable.\n\n"
    for title, predicted, confidence, cut, binary in plans:
        claimed = confidence >= cut[d["rk"]]
        right = (predicted == 0) == (d["y"] == 0) if binary else predicted == d["y"]
        hurt = (predicted == 0) & (d["y"] >= 3)
        lines = []
        for label, mask in by_kind(d, rows, True):
            claim = mask & claimed
            cells = [label, mask.sum(), f"{km[mask].sum():.0f}",
                     "" if label == "all" or not mask.any() else f"{cut[d['rk'][mask][0]]:.2f}",
                     pct(claim.sum(), mask.sum()), pct(km[claim].sum(), km[mask].sum()),
                     pct((claim & right).sum(), claim.sum()), pct(km[claim & right].sum(), km[claim].sum())]  # fmt: skip
            if not binary:
                both = flags(d, predicted)
                cells += [pct((claim & both["right paved or not paved"]).sum(), claim.sum()),
                          pct((claim & both["estimated paved or compacted, truly dirt or rough"]).sum(), claim.sum(), 2)]  # fmt: skip
            lines.append([*cells, pct((claim & hurt).sum(), claim.sum(), 2), pct(km[claim & hurt].sum(), km[claim].sum(), 2)])
        header = ["road kind", "ways", "km", "cut", "claimed, % of ways", "claimed, % of km",
                  "claim right, % of claimed ways", "claim right, % of claimed km"]  # fmt: skip
        if not binary:
            header += ["paved or not right, % of claimed ways",
                       "claimed paved or compacted, truly dirt or rough, % of claimed ways"]  # fmt: skip
        header += ["claimed paved, truly dirt or rough, % of claimed ways", "the same, % of claimed km"]
        out += f"### Claim of {title}\n\n" + table(header, lines)
        if not binary:
            claim = rows & claimed
            out += "These claims by claimed class.\n\n"
            out += table(["claimed class", "ways", "km", "right, % of ways"], [
                [surface, (claim & (predicted == i)).sum(), f"{km[claim & (predicted == i)].sum():.0f}",
                 pct((claim & (predicted == i) & (d["y"] == i)).sum(), (claim & (predicted == i)).sum())]
                for i, surface in enumerate(CLASSES)])  # fmt: skip
    return out


def neighbour_report(d, rows, all_blocks, estimates):
    visible = seen(all_blocks["strict"])
    lines = []
    for label, mask in (("a tagged way at an end node is visible", rows & visible), ("none is visible", rows & ~visible)):
        right = [pct((estimates[name] == d["y"])[mask].sum(), mask.sum()) for name in ("b own tags", MAIN)]
        lines.append([label, mask.sum(), pct(mask.sum(), rows.sum()), *right])
    return table(["test ways", "ways", "% of ways", "b own tags: right class, % of ways", f"{MAIN}: right class, % of ways"], lines)


def evaluate(title, d, rows, all_blocks, models, modes, cuts, out_dir, slug):
    kind_mode, grade_mode = modes
    estimates = {"a kind": kind_mode[d["kind"]], "a2 kind and tracktype": grade_mode[d["kind"] * 6 + d["grade"]]}
    proba = {}
    for name, spec in SPECS.items():
        proba[name] = probabilities(models[name], design(all_blocks, spec)[0])
        estimates[name] = proba[name].argmax(axis=1)
    out = f"# {title}\n\n{rows.sum()} test ways, {d['km'][rows].sum():.0f} km. Every number is on ways with a usable "
    out += "`surface` tag, which is hidden from the estimator.\n\n"
    out += "Estimators: **a kind** the class with the most length of the road kind; **a2** the class with the most "
    out += "length of the road kind and `tracktype`; **b** a model on the way's own tags; **c** the same with the "
    out += "surface of tagged ways around it, where a whole 2 km square is blind (strict); **c-open** the same where "
    out += "every other tagged way is visible, also the next piece of the same road (an upper bound); **d** c with the "
    out += "land use at the middle of the way.\n\n## Accuracy\n\n"
    out += accuracy_report(d, rows, estimates)
    out += "## With and without a visible neighbour\n\n" + neighbour_report(d, rows, all_blocks, estimates)
    for name in cuts:
        out += f"## Claims only where sure: estimator {name}\n\n" + claims_report(d, rows, proba[name], cuts[name])
    (out_dir / f"{slug}.md").write_text(out)
    (out_dir / f"{slug}_confusion.md").write_text(
        f"# Confusion tables: {title}\n\n" + confusion_report(d, rows, estimates)
    )
    return estimates


def right_by_kind(d, rows, predicted):
    right = predicted == d["y"]
    return {label: pct(d["km"][mask & right].sum(), d["km"][mask].sum()) for label, mask in by_kind(d, rows)}


def population_report(d, all_blocks, model, cuts, test_right):
    """How the ways without a usable surface tag differ from the tagged ways, and what the estimator claims on them."""
    tagged, untagged = d["use"] & (d["y"] >= 0), d["use"] & (d["y"] < 0)
    km = d["km"]
    out = "## Mix of road kinds, % of km\n\n"
    out += table(["road kind", "tagged", "untagged"], [
        [label, pct(km[tagged & mask].sum(), km[tagged].sum()), pct(km[untagged & mask].sum(), km[untagged].sum())]
        for label, mask in by_kind(d, d["use"])])  # fmt: skip
    attributes = {
        "named": d["name"] > 0,
        "in a bicycle route": d["routes"][:, 0] > 0,
        "in a hiking route": d["routes"][:, 2] > 0,
        "in an mtb route": d["routes"][:, 1] > 0,
        "smoothness tag": d["tag_smoothness"] != "",
        "in forest": d["land"] == LAND.index("forest"),
        "in residential land": d["land"] == LAND.index("residential"),
        "in farmland or meadow": (d["land"] == LAND.index("farmland")) | (d["land"] == LAND.index("meadow")),
        "a tagged way at an end node": seen(all_blocks["open"]),
    }
    out += "## Attributes, % of km: tagged / untagged\n\n"
    out += table(["road kind", *attributes], [
        [label, *[f"{pct(km[tagged & mask & a].sum(), km[tagged & mask].sum(), 0)} / "
                  f"{pct(km[untagged & mask & a].sum(), km[untagged & mask].sum(), 0)}" for a in attributes.values()]]
        for label, mask in by_kind(d, d["use"])])  # fmt: skip
    track = d["use"] & (d["kind"] == TRACK)
    out += "## Tracks by `tracktype`, % of km\n\n"
    out += table(["tracktype", "tagged", "untagged"], [
        [f"grade{grade}" if grade else "none", pct(km[tagged & track & (d["grade"] == grade)].sum(), km[tagged & track].sum()),
         pct(km[untagged & track & (d["grade"] == grade)].sum(), km[untagged & track].sum())] for grade in range(6)])  # fmt: skip
    proba = probabilities(model, design(all_blocks, ["own", "neighbour kinds", "open"])[0])
    predicted, confidence = proba.argmax(axis=1), proba.max(axis=1)
    claim, kind_claim = confidence >= cuts["all"], confidence >= cuts["kind"][d["rk"]]
    out += f"## What estimator {MAIN} says on the untagged ways\n\n"
    out += "Every tagged way is visible as a neighbour here, as in production. Nothing here measures whether the "
    out += "estimate is right. 'Right if as tagged' is the test accuracy of the road kind, which is an assumption "
    out += "for untagged ways.\n\n"
    lines = []
    expected = 0.0
    for label, mask in by_kind(d, untagged):
        mix = [pct(km[mask & (predicted == i)].sum(), km[mask].sum(), 0) for i in range(5)]
        claimed_mix = [pct(km[mask & claim & (predicted == i)].sum(), km[mask & claim].sum(), 0) for i in range(5)]
        lines.append([label, mask.sum(), f"{km[mask].sum():.0f}", pct((mask & claim).sum(), mask.sum()),
                      pct(km[mask & claim].sum(), km[mask].sum()), pct(km[mask & kind_claim].sum(), km[mask].sum()),
                      " / ".join(mix), " / ".join(claimed_mix), test_right.get(label, "–")])  # fmt: skip
        if label != "all":
            expected += float(test_right.get(label, "0").replace("–", "0")) * km[mask].sum() / km[untagged].sum()
    out += table(["road kind", "untagged ways", "km", "claimed with one cut, % of ways", "claimed with one cut, % of km",
                  "claimed with the cut of the road kind, % of km",
                  "estimate mix, % of km: " + " / ".join(CLASSES), "mix of the claims with one cut, % of km",
                  "right if as tagged, % of km"], lines)  # fmt: skip
    out += f"With the test accuracy of each road kind and the untagged mix of kinds, {expected:.1f} % of the "
    out += "untagged km would get the right class of five (no claim cut).\n\n"
    return out


def main(cache, out_dir):
    start = time.time()
    cache, out_dir = Path(cache), Path(out_dir)
    regions = {"Freiburg": load(cache / "freiburg.npz", True)}
    for name in ("alsace", "cambridge"):
        if (cache / f"{name}.npz").exists():
            regions[name.capitalize()] = load(cache / f"{name}.npz", False, name == "alsace")
    (out_dir / "coverage.md").write_text(
        "# Coverage of the `surface` tag\n\nAlsace is without the part inside the Freiburg box.\n\n"
        + "".join(coverage_report(name, d) for name, d in regions.items())
    )

    d = regions["Freiburg"]
    all_blocks = blocks(d)
    tagged = d["y"] >= 0
    train, test = tagged & (d["fold"] != 0), tagged & (d["fold"] == 0)
    print(f"Freiburg: {train.sum()} training ways, {test.sum()} held-out ways, features {time.time() - start:.0f} s", flush=True)
    kind_mode = mode_table(d["kind"][train], d["y"][train], d["km"][train], len(KINDS), 0)
    grade_mode = mode_table(d["kind"][train] * 6 + d["grade"][train], d["y"][train], d["km"][train], len(KINDS) * 6, -1)
    grade_mode = np.where(grade_mode < 0, np.repeat(kind_mode, 6), grade_mode)

    models, cuts = {}, {}
    for name, spec in SPECS.items():
        x, _, categorical = design(all_blocks, spec)
        models[name] = fit(x[train], d["y"][train], categorical)
        if name.startswith("d"):
            continue
        proba = np.zeros((d["n"], 5))
        for fold in range(1, 5):
            inner, outer = train & (d["fold"] != fold), train & (d["fold"] == fold)
            proba[outer] = probabilities(fit(x[inner], d["y"][inner], categorical), x[outer])
        cuts[name] = cuts_for(d, train, proba)
        print(f"{name}: fitted, cut {cuts[name]['all']:.3f}, {time.time() - start:.0f} s", flush=True)

    estimates = evaluate(
        "Hold-out test, Freiburg (20 % of the 2 km squares)", d, test, all_blocks, models, (kind_mode, grade_mode), cuts,
        out_dir, "holdout_freiburg",
    )  # fmt: skip

    # The signal: the hold-out accuracy of estimator d without each group of inputs.
    spec = SPECS["d + land use"]
    groups = list(dict.fromkeys(group for name in spec for group in all_blocks[name][2]))
    subsets = {"all": test, "tracks": test & (d["kind"] == TRACK), "paths": test & (d["kind"] == KINDS.index("path")),
               "footways": test & (d["kind"] == KINDS.index("footway"))}  # fmt: skip
    groups.remove("kind")
    plans = [("all inputs (estimator d)", lambda g: True), ("kind only", lambda g: g == "kind")]
    plans += [(f"kind and {group}", lambda g, group=group: g in ("kind", group)) for group in groups]
    plans += [(f"all without {group}", lambda g, group=group: g != group) for group in groups]
    lines = []
    for label, wanted in plans:
        x, _, categorical = design(all_blocks, spec, wanted)
        predicted = fit(x[train], d["y"][train], categorical).predict(x)
        right, right2, bad = flags(d, predicted).values()
        cells = [pct(right[rows].sum(), rows.sum()) for rows in subsets.values()]
        cells += [pct(d["km"][test & right].sum(), d["km"][test].sum()), pct(right2[test].sum(), test.sum()),
                  pct(right2[subsets["tracks"]].sum(), subsets["tracks"].sum()), pct(bad[test].sum(), test.sum(), 2)]  # fmt: skip
        lines.append([label, *cells])
    signal = "# Which inputs carry the signal\n\nHold-out test, Freiburg. Each row fits the model again on a part of "
    signal += "the inputs of estimator d. The surfaces around the way are strict: a whole 2 km square is blind.\n\n"
    signal += table(["inputs", *[f"right class of five, {subset}, % of ways" for subset in subsets],
                     "right class of five, all, % of km", "right paved or not, all, % of ways",
                     "right paved or not, tracks, % of ways",
                     "estimated paved or compacted, truly dirt or rough, all, % of ways"], lines)  # fmt: skip
    signal += "The groups: " + "; ".join(
        f"**{group}**: " + ", ".join(n for name in spec for n, g in zip(all_blocks[name][1], all_blocks[name][2]) if g == group)
        for group in groups) + ".\n"  # fmt: skip
    (out_dir / "signal.md").write_text(signal)
    print(f"signal: {time.time() - start:.0f} s", flush=True)

    population = "# Tagged against untagged ways\n\nThe ways without a usable `surface` tag are the ways that the rule "
    population += "is for. The test cannot see them.\n\n# Freiburg\n\n"
    population += population_report(d, all_blocks, models[MAIN], cuts[MAIN], right_by_kind(d, test, estimates[MAIN]))
    for name, region in regions.items():
        if name == "Freiburg":
            continue
        region_blocks = blocks(region)
        rows = region["use"] & (region["y"] >= 0)
        estimates = evaluate(
            f"Transfer test, {name}: fitted on Freiburg, not changed", region, rows, region_blocks, models,
            (kind_mode, grade_mode), cuts, out_dir, f"transfer_{name.lower()}",
        )  # fmt: skip
        population += f"# {name}\n\n" + population_report(
            region, region_blocks, models[MAIN], cuts[MAIN], right_by_kind(region, rows, estimates[MAIN])
        )
        print(f"{name}: {rows.sum()} test ways, {time.time() - start:.0f} s", flush=True)
    (out_dir / "untagged.md").write_text(population)
    print(f"done in {time.time() - start:.0f} s; tables in {out_dir}")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
