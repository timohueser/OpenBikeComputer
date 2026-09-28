"""Finite synthetic candidate experiment; no network-scale or full-graph guarantee."""

from dataclasses import dataclass
from itertools import combinations
import json
import math
import time


@dataclass(frozen=True)
class Route:
    name: str
    costs: tuple[float, ...]
    edges: tuple[tuple[str, float], ...]
    geometry: tuple[tuple[float, float], ...]


def score(costs, weights):
    return sum(c * w for c, w in zip(costs, weights, strict=True))


def dominates(a, b):
    return all(x <= y for x, y in zip(a, b, strict=True)) and any(
        x < y for x, y in zip(a, b, strict=True)
    )


def linear_regret_2d(full, subset):
    """Exact over nonnegative linear weights for these positive finite 2D vectors.

    Between cost-line intersections, each envelope is linear and their ratio is
    monotone. Endpoints and intersections therefore contain the worst regret.
    """
    if not full or not subset or any(len(p) != 2 or min(p) <= 0 for p in full):
        raise ValueError("Expected nonempty positive 2D cost sets")
    if any(p not in full for p in subset):
        raise ValueError("Subset must belong to the candidate set")
    weights = {0.0, 1.0}
    for a, b in combinations(full, 2):
        denominator = a[0] - a[1] - b[0] + b[1]
        if denominator:
            w = (b[1] - a[1]) / denominator
            if 0 <= w <= 1:
                weights.add(w)
    return max(
        1 - min(score(p, (w, 1 - w)) for p in full)
        / min(score(p, (w, 1 - w)) for p in subset)
        for w in weights
    )


def shared_length_fraction(a, b):
    left, right = dict(a.edges), dict(b.edges)
    shared = sum(min(length, right.get(edge, 0)) for edge, length in left.items())
    return shared / min(sum(left.values()), sum(right.values()))


def point_segment_distance(p, a, b):
    dx, dy = b[0] - a[0], b[1] - a[1]
    denominator = dx * dx + dy * dy
    t = max(0, min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy)
                   / denominator)) if denominator else 0
    return math.hypot(p[0] - a[0] - t * dx, p[1] - a[1] - t * dy)


def separated_length(a, b, radius_m=250, sample_step_m=100):
    """Midpoint quadrature in a local metre coordinate system, not a GIS index."""
    separated = total = 0.0
    for start, end in zip(a.geometry, a.geometry[1:]):
        length = math.dist(start, end)
        count = max(1, math.ceil(length / sample_step_m))
        for i in range(count):
            t = (i + 0.5) / count
            p = tuple(x + t * (y - x) for x, y in zip(start, end))
            nearest = min(point_segment_distance(p, x, y)
                          for x, y in zip(b.geometry, b.geometry[1:]))
            separated += length / count if nearest > radius_m else 0
        total += length
    return separated, separated / total


def distinct(a, b):
    # Illustrative experiment parameters; these are not product acceptance gates.
    shorter = min(sum(length for _, length in r.edges) for r in (a, b))
    if shorter * (1 - shared_length_fraction(a, b)) < 1000:
        return False
    # A long shared approach must not hide a meaningful local pass choice.
    return all(metres >= 1000 for metres, _ in
               (separated_length(a, b), separated_length(b, a)))


def material_tradeoff(a, b):
    better = any(x <= y * 0.8 for x, y in zip(a, b, strict=True) if y > 0)
    worse = any(x > y * 1.1 for x, y in zip(a, b, strict=True))
    return better and worse


def select(primary, candidates, weights):
    """Heuristic selection over supplied candidates; no search completeness claim."""
    selected, rejected = [primary], {}
    base = score(primary.costs, weights)
    for route in sorted(candidates, key=lambda r: (score(r.costs, weights), r.name)):
        if route == primary:
            continue
        ratio = score(route.costs, weights) / base
        tradeoff = material_tradeoff(route.costs, primary.costs)
        if ratio > 1.4:
            reason = "excessive_cost"
        elif tradeoff and not any(dominates(other.costs, route.costs) for other in selected) and all(
            material_tradeoff(route.costs, other.costs) or distinct(route, other)
            for other in selected
        ):
            selected.append(route)
            continue
        elif not all(distinct(route, other) for other in selected):
            reason = "same_corridor"
        elif ratio <= 1.05:
            selected.append(route)
            continue
        else:
            reason = "no_material_tradeoff"
        rejected[route.name] = reason
    return selected, rejected


def corridor(name, offset, climb, rough):
    geometry = ((0, 0), (5000, offset), (10000, 0))
    length = sum(math.dist(a, b) for a, b in zip(geometry, geometry[1:]))
    return Route(name, (length, climb, rough), ((name, length),), geometry)


def fixtures():
    return [
        corridor("primary", 0, 500, 1000),
        corridor("parallel_street", 20, 500, 1000),
        corridor("equal_quality_corridor", 1000, 500, 1000),
        corridor("valley", 3000, 100, 1000),
        corridor("smooth_surface", -2200, 650, 0),
        corridor("bad_detour", 5000, 1000, 2000),
    ]


def experiment():
    started = time.perf_counter()
    points = ((2, 8), (6, 6), (8, 2))
    chosen = min(combinations(points, 2), key=lambda s: linear_regret_2d(points, s))
    routes = fixtures()
    selected, rejected = select(routes[0], routes, (1, 2, 0.2))
    only, _ = select(routes[0], [routes[0], routes[1], routes[-1]], (1, 2, 0.2))
    smooth = corridor("same_corridor_smooth", 20, 700, 0)
    same_corridor, same_rejected = select(routes[0], [routes[0], routes[1], smooth], (1, 2, 0.2))
    # Geographic copies can have identical vectors; regret cannot distinguish them.
    equal_vectors = ((2, 8), (2, 8))
    result = {
        "scope": "Synthetic finite candidates; selection experiment, not graph search",
        "unsupported_tradeoff": {
            "all_nondominated": not any(dominates(a, b) for a in points for b in points),
            "cost_vectors": points,
            "minimum_regret_pair": chosen,
            "linear_regret_over_all_weights": linear_regret_2d(points, chosen),
            "best_with_second_cost_at_most_6": min(p for p in points if p[1] <= 6),
            "constraint_choice_retained": (6, 6) in chosen,
        },
        "equal_vector_geographic_route_removed_without_regret":
            linear_regret_2d(equal_vectors, equal_vectors[:1]) == 0,
        "candidate_filter": {
            "selected": [r.name for r in selected],
            "rejected": rejected,
            "parallel_street_edge_overlap": shared_length_fraction(routes[0], routes[1]),
            "parallel_street_spatial_separation": separated_length(routes[1], routes[0]),
            "only_one_case": [r.name for r in only],
            "same_corridor_tradeoff_case": [r.name for r in same_corridor],
            "same_corridor_tradeoff_rejected": same_rejected,
        },
        "limitations": [
            "No real map, via-node generation, turn state, or network transfer is tested",
            "Spatial separation uses sampled planar geometry and pairwise filtering",
            "Substantive metric tradeoffs can qualify without geographic separation",
            "Geographic difference uses absolute changed length, not a whole-route fraction",
            "Metric tradeoffs use whole-route totals; local metric significance is not modeled",
            "Cost and separation thresholds are illustrative, not rider-validated",
            "Regret is exact only over the supplied positive 2D candidate vectors",
            "Selection does not prove local optimality or uniformly bounded stretch",
        ],
    }
    result["elapsed_ms"] = (time.perf_counter() - started) * 1000
    return result


if __name__ == "__main__":
    print(json.dumps(experiment(), indent=2))
