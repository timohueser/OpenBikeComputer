import ast
from dataclasses import replace
import math
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
import zlib

import numpy as np
from scipy.optimize import minimize

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from data import intervals, parse_record, read_rides, split_for, valid_mask
from model import Config, Intervals, Layout, Learner, Live
from replay import MODELS, forecast_rows


class LearnerContracts(unittest.TestCase):
    def test_streaming_summary_matches_independent_quadratic_elimination(self):
        cfg = Config()
        layout = Layout(cfg, bikes=2, surfaces=2)
        learner = Learner(layout, np.float64)
        rng = np.random.default_rng(91)
        rows, targets, weights = [], [], []
        for _ in range(120):
            x = layout.features([rng.uniform(-0.1, 0.1)], int(rng.integers(2)), int(rng.integers(2)))[0]
            z, w = rng.uniform(-0.4, 0.4), rng.uniform(0.01, 0.2)
            learner.observe(x, 0.0, math.exp(z), w)
            rows.append(x); targets.append(z); weights.append(w)
        X, y, w = np.array(rows, dtype=float), np.array(targets), np.array(weights)
        w /= w.sum()
        mean_x, mean_y = w @ X, float(w @ y)
        expected_Q = X.T @ (w[:, None] * X) - (1 - cfg.eta) * np.outer(mean_x, mean_x)
        expected_h = X.T @ (w * y) - (1 - cfg.eta) * mean_x * mean_y
        A, h, H, lower, upper = learner.problem()
        np.testing.assert_allclose(layout.matrix(A), expected_Q, atol=1e-12)
        np.testing.assert_allclose(h, expected_h, atol=1e-12)
        self.assertGreater(np.linalg.eigvalsh(H).min(), 0)
        reference = minimize(lambda x: 0.5 * x @ H @ x - h @ x, learner.theta.copy(),
                             jac=lambda x: H @ x - h, bounds=list(zip(lower, upper)),
                             method="L-BFGS-B", options={"ftol": 1e-14, "gtol": 1e-10})
        self.assertTrue(reference.success)
        self.assertTrue(learner.finish())
        obtained = 0.5 * learner.theta @ H @ learner.theta - h @ learner.theta
        self.assertLessEqual(obtained - reference.fun, 1e-7)

    def test_tiny_followup_evidence_cannot_unlock_a_full_step(self):
        cfg = Config()
        learner = Learner(Layout(cfg, gradient=False))
        x = np.ones(1, dtype=np.float32)
        learner.observe(x, 0, 2, 10)
        learner.finish()
        before = learner.theta.copy()
        learner.reset_ride()
        learner.observe(x, 0, 2, 0.001)
        learner.finish()
        self.assertLessEqual(abs(float(learner.theta[0] - before[0])), cfg.delta_global * 0.001 / 10 + 1e-8)
        saved = (learner.theta.copy(), learner.A.copy(), learner.g.copy())
        learner.reset_ride()
        self.assertFalse(learner.finish())
        for actual, expected in zip((learner.theta, learner.A, learner.g), saved):
            np.testing.assert_array_equal(actual, expected)

    def test_invalid_candidate_preserves_persistent_state(self):
        learner = Learner(Layout(Config()))
        x = learner.layout.features([0])[0]
        self.assertFalse(learner.observe(x, 0, float("nan"), 1))
        learner.observe(x, 0, 1e100, 10)
        saved = (learner.theta.copy(), learner.A.copy(), learner.g.copy())
        learner.C[0] = np.nan
        self.assertFalse(learner.finish())
        for actual, expected in zip((learner.theta, learner.A, learner.g), saved):
            np.testing.assert_array_equal(actual, expected)

    def test_float32_matches_float64_over_repeated_rides(self):
        cfg, rng = Config(), np.random.default_rng(7)
        layout = Layout(cfg, bikes=2, surfaces=2)
        learners = [Learner(layout, dtype) for dtype in (np.float32, np.float64)]
        for ride in range(35):
            for learner in learners:
                learner.reset_ride()
            for _ in range(30):
                x = layout.features([rng.uniform(-0.1, 0.1)], ride % 2, (ride // 2) % 2)[0]
                pace = math.exp(0.2 + rng.normal(0, 0.15))
                for learner in learners:
                    learner.observe(x, 0, pace, 0.4)
            for learner in learners:
                self.assertTrue(learner.finish())
        np.testing.assert_allclose(learners[0].theta, learners[1].theta, atol=3e-5)
        self.assertEqual(learners[0].state_bytes, 4 * (layout.size * (layout.size + 1) + 4 * layout.size + 2))

    def test_gradient_clamping_and_bike_transfer_are_explicit(self):
        cfg = Config()
        layout = Layout(cfg, bikes=2)
        model = Learner(layout)
        model.theta[1] = math.log(1.2)
        for gradient in (0, 0.08):
            road = layout.features([gradient], bike=0)[0]
            mtb = layout.features([gradient], bike=1)[0]
            self.assertAlmostEqual(math.exp(float((mtb - road) @ model.theta)), 1.2, places=6)
        np.testing.assert_array_equal(layout.features([0.5]), layout.features([0.2]))
        with self.assertRaises(ValueError):
            layout.features([0], bike=2)


class ForecastContracts(unittest.TestCase):
    def test_live_half_life_and_full_route_multiplier(self):
        live = Live(Config())
        for _ in range(10):
            live.observe(0.5, 0.5 / 1.3)
        self.assertAlmostEqual(math.exp(float(live.u)), math.sqrt(1.3), places=6)
        saved = live.u
        live.observe(0, 1)
        self.assertEqual(live.u, saved)
        cfg = replace(Config(), target_km=(1.0, 10.0))
        values = np.array([np.full(30, 0.5), np.ones(30), np.zeros(30), np.zeros(30)])
        baseline = {name: np.ones(30) for name in MODELS[:4]}
        rows = list(forecast_rows(cfg, values, 0, 1, baseline, {"grade_live": 1.3}, None,
                                  {name: set() for name in MODELS}))
        adjusted = [r for r in rows if r["model"] == "grade_live"]
        self.assertAlmostEqual(adjusted[0]["predicted_minutes"], 2 * 1.3)
        self.assertAlmostEqual(adjusted[1]["predicted_minutes"], 20 * 1.3)

    def test_future_times_do_not_change_issued_prediction_or_slot_selection(self):
        cfg = replace(Config(), target_km=(1.0, 3.0))
        values = np.array([np.full(10, 0.5), np.ones(10), np.zeros(10), np.zeros(10)])
        baseline = {name: np.ones(10) for name in MODELS[:4]}
        def issue(data):
            return list(forecast_rows(cfg, data, 0, 0, baseline, {}, None, {name: set() for name in MODELS}))
        original = issue(values)
        values[1, :] = 1e6
        values[3, :] = 4
        changed = issue(values)
        self.assertEqual(original, changed)
        chosen = [row for row in changed if row["model"] == "grade_live"]
        self.assertEqual([row["calibration_slot"] for row in chosen], [1, 0])

    def test_interval_buffer_matches_weighted_distribution_and_keeps_extreme_errors(self):
        cfg = replace(Config(), buffer_size=2, reference_weight=2)
        reference = [[-0.2, 0.2]] * 6
        model = Intervals(cfg, reference)
        for error in (0.4, 5.0, 0.1):
            model.add_ride({0: error})
        self.assertEqual(model.count[0], 2)
        self.assertIn(5.0, model.errors[0])
        self.assertAlmostEqual(float(model.bounds[0, 0]), -0.2, places=6)
        self.assertAlmostEqual(float(model.bounds[0, 1]), 5.0, places=6)


class DataContracts(unittest.TestCase):
    def test_overlaps_cannot_learn_from_a_previous_records_future(self):
        with tempfile.TemporaryDirectory() as folder:
            database = Path(folder) / "rides.sqlite"
            with sqlite3.connect(database) as db:
                db.execute("CREATE TABLE rides (user, ride, start, sport, split, points, data)")
                for ride, start, times in ((1, 0, [2, -1]), (2, 90, [1, 1]), (3, 120, [1, 1])):
                    values = np.array([[0.1, 0.1], times, [0, 0], [0, 0]], dtype="<f4")
                    db.execute("INSERT INTO rides VALUES (?, ?, ?, ?, ?, ?, ?)",
                               (1, ride, start, "bike", "test", 3, zlib.compress(values.tobytes())))
            from collections import Counter
            audit = Counter()
            self.assertEqual([row[1] for row in read_rides(database, "test", audit=audit)], [1, 3])
            self.assertEqual(audit["overlapping_records_excluded"], 1)

    def test_parser_never_executes_literals_and_preserves_quoted_strings(self):
        for text in ("{'sport': 'bike', 'values': [1, 2.3]}", "{'name': 'a\\\'b'}", '{"name": "a"}'):
            self.assertEqual(parse_record(text), ast.literal_eval(text))
        with self.assertRaises((ValueError, SyntaxError)):
            parse_record("__import__('os').getcwd()")

    def test_gap_is_not_movement_evidence(self):
        row = dict(latitude=[50, 50.001, 50.002], longitude=[10, 10, 10],
                   altitude=[100, 101, 102], timestamp=[0, 20, 620])
        values = intervals(row)
        self.assertEqual(valid_mask(values).tolist(), [True, False])
        self.assertEqual(split_for(19), split_for(19))


if __name__ == "__main__":
    unittest.main()
