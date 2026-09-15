"""Bounded-state log-pace learner and empirical prediction intervals."""

from dataclasses import dataclass
import math

import numpy as np


@dataclass(frozen=True)
class Config:
    anchors: tuple = (-0.20, -0.10, -0.05, -0.02, 0.0, 0.02, 0.05, 0.10, 0.20)
    initial_pace: tuple = (2.4, 1.8, 1.9, 2.3, 3.0, 4.0, 6.2, 10.0, 18.0)
    live_half_minutes: float = 5.0
    history_half_rides: float = 20.0
    ride_cap_km: float = 10.0
    eta: float = 0.25
    clip_log: float = math.log(2.0)
    live_min: float = math.log(0.5)
    live_max: float = math.log(2.5)
    lambda_global: float = 0.1
    lambda_detail: float = 1.0
    delta_global: float = 0.12
    delta_detail: float = 0.04
    absolute_global: float = math.log(3.0)
    absolute_detail: float = math.log(2.0)
    sweeps: int = 8
    buffer_size: int = 32
    reference_weight: float = 32.0
    reference_knots: int = 64
    established_minutes: float = 10.0
    duration_boundaries: tuple = (10.0, 30.0)
    target_km: tuple = (1.0, 3.0, 10.0, 30.0)
    max_rides: int = 60


class Layout:
    def __init__(self, config, bikes=1, surfaces=1, gradient=True):
        self.config, self.bikes, self.surfaces, self.gradient = config, bikes, surfaces, gradient
        self.reference_anchor = config.anchors.index(0.0)
        self.grade_offset = 1 + bikes - 1 + surfaces - 1
        self.size = self.grade_offset + (len(config.anchors) - 1 if gradient else 0)
        self.triangle = np.tril_indices(self.size)

    def features(self, grades, bike=0, surface=0):
        if not (0 <= bike < self.bikes and 0 <= surface < self.surfaces):
            raise ValueError("unknown category")
        grades = np.atleast_1d(grades)
        result = np.zeros((len(grades), self.size), dtype=np.float32)
        result[:, 0] = 1
        if bike:
            result[:, bike] = 1
        if surface:
            result[:, self.bikes + surface - 1] = 1
        if self.gradient:
            anchors = np.array(self.config.anchors)
            bounded = np.clip(grades, anchors[0], anchors[-1])
            left = np.clip(np.searchsorted(anchors, bounded, side="right") - 1, 0, len(anchors) - 2)
            weight = (bounded - anchors[left]) / (anchors[left + 1] - anchors[left])
            for index in range(len(anchors)):
                if index == self.reference_anchor:
                    continue
                column = self.grade_offset + index - int(index > self.reference_anchor)
                result[:, column] = (left == index) * (1 - weight) + (left + 1 == index) * weight
        return result

    def initial_log(self, grades):
        return np.interp(grades, self.config.anchors, np.log(self.config.initial_pace)).astype(np.float32)

    def matrix(self, packed):
        result = np.zeros((self.size, self.size), dtype=packed.dtype)
        result[self.triangle] = packed
        result[(self.triangle[1], self.triangle[0])] = packed
        return result


class Learner:
    def __init__(self, layout, dtype=np.float32):
        self.layout, self.config, self.dtype = layout, layout.config, dtype
        size, packed = layout.size, len(layout.triangle[0])
        self.theta = np.zeros(size, dtype=dtype)
        self.A = np.zeros(packed, dtype=dtype)
        self.g = np.zeros(size, dtype=dtype)
        self.C = np.zeros(packed, dtype=dtype)
        self.v = np.zeros(size, dtype=dtype)
        self.m = np.zeros(size, dtype=dtype)
        self.nw = np.zeros(2, dtype=dtype)
        self.rejections = 0

    @property
    def state_bytes(self):
        return sum(x.nbytes for x in (self.theta, self.A, self.g, self.C, self.v, self.m, self.nw))

    def reset_ride(self):
        for value in (self.C, self.v, self.m, self.nw):
            value.fill(0)

    def observe(self, x, initial_log, pace, distance):
        if not (math.isfinite(pace) and math.isfinite(distance) and pace > 0 and distance > 0
                and math.isfinite(initial_log) and np.isfinite(x).all()):
            self.rejections += 1
            return False
        x = np.asarray(x, dtype=self.dtype)
        prediction = float(x @ self.theta)
        target = prediction + np.clip(math.log(pace) - initial_log - prediction,
                                      -self.config.clip_log, self.config.clip_log)
        a = self.dtype(distance / (float(self.nw[1]) + distance))
        dx, dz = x - self.m, self.dtype(target - self.nw[0])
        i, j = self.layout.triangle
        self.C[:] = (1 - a) * (self.C + a * dx[i] * dx[j])
        self.v[:] = (1 - a) * (self.v + a * dx * dz)
        self.m[:] += a * dx
        self.nw[0] += a * dz
        self.nw[1] += self.dtype(distance)
        return True

    def problem(self):
        """Post-ride workspace; the persistent state remains unchanged."""
        if self.nw[1] <= 0:
            return None
        cfg, size = self.config, self.layout.size
        s = min(float(self.nw[1]) / cfg.ride_cap_km, 1.0)
        rho = self.dtype(2 ** (-s / cfg.history_half_rides))
        i, j = self.layout.triangle
        A = rho * self.A + self.dtype(s) * (self.C + cfg.eta * self.m[i] * self.m[j])
        g = rho * self.g + self.dtype(s) * (self.v + cfg.eta * self.m * self.nw[0])
        H = self.layout.matrix(A)
        penalty = np.full(size, cfg.lambda_detail, dtype=self.dtype)
        penalty[0] = cfg.lambda_global
        H[np.diag_indices(size)] += penalty
        delta = np.full(size, s * cfg.delta_detail, dtype=self.dtype)
        delta[0] = s * cfg.delta_global
        absolute = np.full(size, cfg.absolute_detail, dtype=self.dtype)
        absolute[0] = cfg.absolute_global
        lower = np.maximum(-absolute, self.theta - delta)
        upper = np.minimum(absolute, self.theta + delta)
        return A, g, H, lower, upper

    def finish(self):
        problem = self.problem()
        if problem is None:
            return False
        A, g, H, lower, upper = problem
        if (not all(np.isfinite(x).all() for x in problem)
                or np.any(np.diag(H) <= 0) or np.any(lower > upper)):
            self.rejections += 1
            return False
        candidate = self.theta.copy()
        before = float(0.5 * candidate @ H @ candidate - g @ candidate)
        if not math.isfinite(before):
            self.rejections += 1
            return False
        for _ in range(self.config.sweeps):
            for j in range(self.layout.size):
                other = H[j] @ candidate - H[j, j] * candidate[j]
                candidate[j] = np.clip((g[j] - other) / H[j, j], lower[j], upper[j])
        after = float(0.5 * candidate @ H @ candidate - g @ candidate)
        if (not np.isfinite(candidate).all() or not math.isfinite(after)
                or after > before + 2e-6 * max(1.0, abs(before))):
            self.rejections += 1
            return False
        self.theta[:] = candidate
        self.A[:], self.g[:] = A, g
        return True


class Live:
    def __init__(self, config):
        self.config = config
        self.u = np.float32(0)
        self.accepted_minutes = np.float32(0)

    def observe(self, actual, baseline):
        if not (actual > 0 and baseline > 0 and math.isfinite(actual) and math.isfinite(baseline)):
            return
        cfg = self.config
        alpha = 1 - 2 ** (-actual / cfg.live_half_minutes)
        innovation = np.clip(math.log(actual / baseline) - float(self.u), -cfg.clip_log, cfg.clip_log)
        self.u = np.float32(np.clip(self.u + alpha * innovation, cfg.live_min, cfg.live_max))
        self.accepted_minutes += np.float32(actual)


def group_for(config, phase, predicted):
    return phase * 3 + int(np.searchsorted(config.duration_boundaries, predicted, side="right"))


class Intervals:
    def __init__(self, config, reference):
        self.config = config
        self.reference = reference
        self.errors = np.zeros((6, config.buffer_size), dtype=np.float32)
        self.count = np.zeros(6, dtype=np.uint16)
        self.cursor = np.zeros(6, dtype=np.uint16)
        self.bounds = np.zeros((6, 2), dtype=np.float32)
        self.refresh()

    @property
    def state_bytes(self):
        return sum(x.nbytes for x in (self.errors, self.count, self.cursor, self.bounds))

    def refresh(self):
        for group in range(6):
            ref = np.asarray(self.reference[group], dtype=np.float32)
            if len(ref) == 0 or not np.isfinite(ref).all():
                raise ValueError("Calibration reference must be finite and nonempty")
            personal = self.errors[group, :self.count[group]]
            values = np.r_[ref, personal]
            weights = np.r_[np.full(len(ref), self.config.reference_weight / len(ref)), np.ones(len(personal))]
            order = np.argsort(values, kind="stable")
            mass = np.cumsum(weights[order]) / weights.sum()
            self.bounds[group] = values[order][np.searchsorted(mass, [0.05, 0.95])]

    def add_ride(self, errors):
        for group, error in errors.items():
            if not math.isfinite(error):
                raise ValueError("Nonfinite calibration outcome")
            self.errors[group, self.cursor[group]] = error
            self.cursor[group] = (self.cursor[group] + 1) % self.config.buffer_size
            self.count[group] = min(int(self.count[group]) + 1, self.config.buffer_size)
        self.refresh()

    def predict(self, phase, predicted):
        group = group_for(self.config, phase, predicted)
        low, high = self.bounds[group]
        return predicted * math.exp(float(low)), predicted * math.exp(float(high))
