"""Scalar specialization of the bounded personal log-pace learner."""

import math

import numpy as np


class Scalar:
    def __init__(self, config):
        self.config = config
        # Personal correction, historical curvature/target, ride mean/distance.
        self.state = np.zeros(5, dtype=np.float32)
        self.rejections = 0

    @property
    def theta(self):
        return float(self.state[0])

    def reset_ride(self):
        self.state[3:] = 0

    def observe(self, initial_log, pace, distance):
        if not (all(map(math.isfinite, (initial_log, pace, distance))) and pace > 0 and distance > 0
                and np.isfinite(self.state).all()):
            self.rejections += 1
            return False
        theta, _, _, mean, weight = map(float, self.state)
        target = theta + np.clip(math.log(pace)-initial_log-theta, -self.config.clip_log, self.config.clip_log)
        total = weight + distance
        if total > np.finfo(np.float32).max:
            self.rejections += 1
            return False
        self.state[3] = mean + (distance/total)*(target-mean)
        self.state[4] = total
        return True

    def finish(self):
        if not np.isfinite(self.state).all():
            self.rejections += 1
            return False
        theta, A, h, mean, weight = map(float, self.state)
        if weight <= 0:
            return False
        cfg = self.config
        s = min(weight/cfg.ride_cap_km, 1.)
        rho = 2**(-s/cfg.history_half_rides)
        A, h = np.float32(rho*A+s*cfg.eta), np.float32(rho*h+s*cfg.eta*mean)
        H = np.float32(A+cfg.lambda_global)
        lower = max(-cfg.absolute_global, theta-s*cfg.delta_global)
        upper = min(cfg.absolute_global, theta+s*cfg.delta_global)
        if not all(map(math.isfinite, (A, h, H, lower, upper))) or H <= 0 or lower > upper:
            self.rejections += 1
            return False
        candidate = np.float32(np.clip(h/H, lower, upper))
        before = .5*float(H)*theta*theta-float(h)*theta
        after = .5*float(H)*float(candidate)**2-float(h)*float(candidate)
        if not math.isfinite(after) or after > before + 2e-6*max(1., abs(before)):
            self.rejections += 1
            return False
        self.state[:3] = candidate, A, h
        self.reset_ride()
        return True
