"""Small shared log-pace corrections; personal state stays in the scalar learner."""

from collections import Counter
import math

import numpy as np
from scipy.optimize import lsq_linear

from replay import learn_mask

NAMES = ("regional_offset", "mtb", "track", "path", "cycleway", "firm", "soft", "rough", "unpaved")
MODELS = {"gradient": 1, "bike": 2, "path": 5, "surface": 9}
SURFACES = {
    "firm": {"compacted", "fine_gravel", "gravel", "pebblestone"},
    "soft": {"ground", "dirt", "earth", "grass", "sand", "mud", "clay"},
    "rough": {"cobblestone", "sett", "unhewn_cobblestone", "rock", "stone"},
    "unpaved": {"unpaved"},
}
POLICY = dict(ridge=.25, mean_weight=.25, sample_log_bound=math.log(3),
              correction_log_bound=math.log(2), regional_log_bound=math.log(1.5))


def features(tags, sport):
    result = np.zeros((len(tags), len(NAMES)))
    result[:, 0] = 1
    result[:, 1] = sport == "mountain bike"
    for i, row in enumerate(tags):
        highway = row.get("highway", {})
        result[i, 2] = highway.get("track", 0)
        result[i, 3] = sum(highway.get(value, 0) for value in ("path", "footway", "bridleway", "steps"))
        result[i, 4] = highway.get("cycleway", 0)
        for j, values in enumerate(SURFACES.values(), 5):
            result[i, j] = sum(row.get("surface", {}).get(value, 0) for value in values)
    return result


def correction(x, theta):
    # A total cap also bounds combinations of separately bounded coefficients.
    return np.clip(x[:, :len(theta)] @ theta, -math.log(3), math.log(3))


def fit(rides, layout):
    """Rider-balanced bounded ridge fit, with reduced between-ride influence.

    Every rider has unit total distance weight, divided equally over learnable rides.
    Center each ride's design and target, retaining sqrt(eta) of the ride mean.
    The sample target is clipped before fitting. There is no hyperparameter search.
    """
    prepared = []
    for item, values, matched in rides:
        mask = learn_mask(values, 30)
        if not mask.any():
            continue
        x = features(matched["causal"], item["sport"])[mask]
        y = np.log(values[1, mask]/values[0, mask]) - layout.initial_log(values[2, mask])
        y = np.clip(y, -POLICY["sample_log_bound"], POLICY["sample_log_bound"])
        w = values[0, mask].astype(float)
        w /= w.sum()
        scale = 1 - math.sqrt(POLICY["mean_weight"])
        x -= scale * (w @ x)
        y -= scale * (w @ y)
        prepared.append((item["user"], x, y, w))
    counts = Counter(user for user, *_ in prepared)
    x = np.concatenate([a * np.sqrt(w/counts[user])[:, None] for user, a, _, w in prepared])
    y = np.concatenate([b * np.sqrt(w/counts[user]) for user, _, b, w in prepared])
    result = {}
    for name, size in MODELS.items():
        ridge = np.full(size, POLICY["ridge"])
        ridge[0] = .1
        design = np.vstack([x[:, :size], np.diag(np.sqrt(ridge))])
        target = np.r_[y, np.zeros(size)]
        bound = np.full(size, POLICY["correction_log_bound"])
        bound[0] = POLICY["regional_log_bound"]
        solution = lsq_linear(design, target, bounds=(-bound, bound), tol=1e-10)
        if not solution.success or not np.isfinite(solution.x).all():
            raise ValueError("Shared fit failed")
        result[name] = solution.x.tolist()
    return dict(coefficients=result, policy=POLICY, features=NAMES,
                training_riders=len(counts), training_rides=len(prepared))
