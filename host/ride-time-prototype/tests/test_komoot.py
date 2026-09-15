"""Authored time accounting, pushing, and source-boundary cases."""

import json
from pathlib import Path
import sys
import tempfile
import unittest

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from komoot_data import adapt, pause_overlap, prepare, read_gpx
from komoot_protocol import split


def sample(seconds, metres, altitude=None):
    points = np.column_stack([seconds, np.zeros(len(seconds)),
                              np.degrees(np.asarray(metres)/6371000),
                              np.zeros(len(seconds)) if altitude is None else altitude])
    return points, np.zeros(len(seconds), dtype=int)


def metadata(duration, events=()):
    return dict(date="1970-01-01T00:00:00Z", duration=duration,
                time_in_motion=duration, pause_events=list(events))


class KomootTests(unittest.TestCase):
    def test_pause_union_and_partial_boundary(self):
        seconds = np.array([0., 10, 20, 30, 40])
        events = [dict(start_ms=5000, end_ms=15000), dict(start_ms=10000, end_ms=25000)]
        np.testing.assert_allclose(pause_overlap(seconds, events, 0, 40), [5, 10, 5, 0])
        with self.assertRaisesRegex(ValueError, "invalid_pause"):
            pause_overlap(seconds, [dict(start_ms=0, end_ms=41001)], 0, 40)

    def test_slow_pushing_is_not_a_stop(self):
        p, s = sample([0, 10, 20, 30], [0, 1, 2, 3])  # 0.36 km/h
        arrays, audit = adapt(p, s, metadata(30))
        self.assertAlmostEqual(audit["moving_minutes"], .5)
        self.assertEqual(audit["unknown_minutes"], 0)
        self.assertAlmostEqual(arrays["raw"][0].sum(), .003)

    def test_identical_gap_endpoints_are_still_unknown(self):
        p, s = sample([0, 10, 610, 620], [0, 20, 20, 40])
        arrays, audit = adapt(p, s, metadata(620))
        self.assertEqual(audit["unknown_minutes"], 10)
        self.assertFalse(audit["point_eligible"])
        self.assertFalse(arrays["learn"][2])
        self.assertAlmostEqual(audit["moving_minutes"], 1/3)

    def test_explicit_pause_recovers_gap_without_fabricating_pace(self):
        p, s = sample([0, 10, 620, 630], [0, 20, 40, 60])
        m = metadata(630, [dict(start_ms=10000, end_ms=610000)])
        m["time_in_motion"] = 30
        arrays, audit = adapt(p, s, m)
        self.assertEqual(audit["unknown_minutes"], 0)
        self.assertEqual(audit["explicit_pause_minutes"], 10)
        self.assertAlmostEqual(audit["moving_minutes"], .5)
        self.assertFalse(arrays["learn"][1])
        np.testing.assert_allclose(arrays["elapsed_minutes"], arrays["pause_minutes"]+arrays["raw"][1])

    def test_segment_boundary_and_speed_spike_are_not_joined(self):
        p, s = sample([0, 10, 20, 30], [0, 20, 1000, 1020])
        arrays, audit = adapt(p, s, metadata(30))
        self.assertEqual(audit["speed_flags"], 1)
        self.assertEqual(audit["unknown_intervals"], 1)
        self.assertFalse(arrays["learn"][2])
        p, s = sample([0, 10, 20, 30], [0, 20, 40, 60])
        s[2:] = 1
        arrays, audit = adapt(p, s, metadata(30))
        self.assertEqual(audit["segment_breaks"], 1)
        self.assertEqual(arrays["raw"][0, 1], 0)

    def test_grade_is_causal_and_resets_after_unknown_gap(self):
        p, s = sample([0, 10, 20, 620, 630, 640], [0, 20, 40, 60, 80, 100], [0, 1, 2, 40, 41, 42])
        a, _ = adapt(p, s, metadata(640))
        changed = p.copy()
        changed[-1, 3] += 100
        b, _ = adapt(changed, s, metadata(640))
        np.testing.assert_array_equal(a["raw"][:, :4], b["raw"][:, :4])
        self.assertAlmostEqual(a["raw"][2, 3], .05)

    def test_proxy_coverage_does_not_relabel_unknown_time_as_a_break(self):
        seconds = np.arange(61)*10.
        seconds[30:] += 120
        metres = np.arange(61)*20.
        metres[30:] -= 19
        p, segments = sample(seconds, metres)
        m = metadata(720)
        m["time_in_motion"] = 590
        _, audit = adapt(p, segments, m)
        self.assertTrue(audit["proxy_eligible"])
        self.assertFalse(audit["point_eligible"])
        self.assertAlmostEqual(audit["unknown_minutes"], 130/60)
        self.assertEqual(audit["explicit_pause_minutes"], 0)
        m["time_in_motion"] = 400
        self.assertFalse(adapt(p, segments, m)[1]["proxy_eligible"])

    def test_split_excludes_a_ride_crossing_the_holdout_boundary(self):
        from komoot_data import timestamp
        cutoff = timestamp("2024-01-01T00:00:00Z")
        rides = [dict(id="before", start=cutoff-20, end=cutoff-10),
                 dict(id="crossing", start=cutoff-10, end=cutoff+10),
                 dict(id="after", start=cutoff, end=cutoff+10)]
        self.assertEqual(split(rides), dict(warmup=["before"], evaluation=["after"],
                                            crosses_cutoff=["crossing"]))

    def test_missing_field_and_naive_timestamp_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            p = Path(directory)/"ride.gpx"
            for time, elevation in (("2020-01-01T00:00:00", "<ele>1</ele>"),
                                    ("2020-01-01T00:00:00Z", "")):
                p.write_text(f'<gpx><trk><trkseg><trkpt lat="0" lon="0"><time>{time}</time>{elevation}</trkpt></trkseg></trk></gpx>')
                with self.assertRaises(ValueError):
                    read_gpx(p)

    def test_source_hash_checked_before_preparation(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            (source/"downloads").mkdir()
            (source/"downloads/1.gpx").write_text("changed")
            (source/"manifest.json").write_text(json.dumps(dict(complete=True, errors=[],
                rides=[dict(id="1")], files=[dict(id="1", path="downloads/1.gpx", sha256="wrong")])) )
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                prepare(source, source/"prepared")
            self.assertFalse((source/"prepared").exists())


if __name__ == "__main__":
    unittest.main()
