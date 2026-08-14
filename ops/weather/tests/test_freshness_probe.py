#!/usr/bin/env python3
"""Self-tests for `ops/weather/freshness_probe.py` (WXR8 #1247).

The probe is the only thing that notices the weather service has stopped being usable or has
started costing money, it runs unattended from a GitHub schedule, and until this file it had no
test of any kind — its correctness was "it printed something sensible the day it was written".
Every case here is one alarm either firing or staying quiet, driven through `probe()` exactly as
the workflow drives it.

The input is `specs/vectors/wx-manifest-v2.json`, the same cross-language fixture the baker and both
clients are pinned against, mutated per case. That matters: a probe tested against a document only
its own tests believe in would drift away from what the service actually publishes.

    python3 -m unittest discover -s ops/weather/tests -v
"""

from __future__ import annotations

import io
import json
import sys
import tempfile
import unittest
import urllib.error
from contextlib import contextmanager, redirect_stdout
from pathlib import Path

OPS = Path(__file__).resolve().parents[1]
REPO = OPS.parents[1]
sys.path.insert(0, str(OPS))

import freshness_probe as probe  # noqa: E402

FIXTURE = REPO / "specs/vectors/wx-manifest-v2.json"
# The fixture's generation is 20260810T1430Z, generated 14:31:07.
HEALTHY_NOW = "2026-08-10T14:35:00Z"


def run(document: dict, *extra: str) -> tuple[int, str]:
    """Run the probe over a document, as a local manifest. Returns (exit code, printed report)."""
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "manifest.json"
        path.write_text(json.dumps(document), encoding="utf-8")
        args = probe.build_parser().parse_args(["--manifest", str(path), "--now", HEALTHY_NOW, *extra])
        buffer = io.StringIO()
        with redirect_stdout(buffer):
            code = probe.probe(args)
        return code, buffer.getvalue()


def healthy() -> dict:
    return json.loads(FIXTURE.read_text(encoding="utf-8"))


@contextmanager
def fake_head(answer):
    """Stand in for the one HEAD `check_sweep` issues. `answer(url)` returns the fetch tuple or
    raises, which is the whole of the branch space that check has."""
    real = probe.fetch
    probe.fetch = lambda url, timeout, method="GET": answer(url)
    try:
        yield
    finally:
        probe.fetch = real


@contextmanager
def http_status(code: str):
    """An `HTTPError` raiser whose error object is closed afterwards — `HTTPError` is file-like, and
    leaking one turns every run of this suite into a `ResourceWarning`."""
    error = urllib.error.HTTPError("https://wx.example/probe", code, "test", None, io.BytesIO(b""))

    def raise_it(_url):
        raise error

    try:
        yield raise_it
    finally:
        error.close()


class ProbeTests(unittest.TestCase):
    def test_the_shared_fixture_is_a_healthy_service(self):
        """The baseline every other case is a mutation of. If this ever fails, the fixture moved
        and the probe is measuring something that no longer exists."""
        code, output = run(healthy())
        self.assertEqual(code, 0, output)
        self.assertNotIn("ALERTS", output)
        self.assertIn("generation 20260810T1430Z, keeping 2 previous", output)

    def test_an_unsupported_document_version_is_not_interpreted(self):
        """Interpreting a document whose fields mean something else would report nonsense."""
        self.assertEqual(probe.SUPPORTED_MANIFEST_VERSIONS, {2})
        document = healthy()
        document["version"] = 99
        code, output = run(document)
        self.assertEqual(code, 1)
        self.assertIn("UNSUPPORTED", output)

    def test_the_manifest_path_is_the_v2_tree(self):
        """The public manifest path is a constant rather than an operator flag."""
        self.assertEqual(probe.MANIFEST_PATH, "wx/v2/manifest.json")
        self.assertEqual(
            probe.manifest_url("https://wx.openbikecomputer.com"),
            "https://wx.openbikecomputer.com/wx/v2/manifest.json",
        )

    def test_a_dead_timer_shows_up_as_a_stale_manifest(self):
        """With one dataset on one timer, "the manifest is fresh" and "the timer is alive" are the
        same statement — the timer that would be dead is the one that writes the manifest. This is
        why no separate per-source timer check is required."""
        document = healthy()
        document["generated_at"] = "2026-08-10T13:00:00Z"
        code, output = run(document)
        self.assertEqual(code, 1)
        self.assertIn("nothing has run on the box", output)

    def test_an_expired_generation_alerts_that_riders_have_no_weather(self):
        document = healthy()
        document["freshness"]["stale_after"] = "2026-08-10T12:00:00Z"
        code, output = run(document)
        self.assertEqual(code, 1)
        self.assertIn("went unusable", output)
        self.assertIn("not the same as no rain", output)

    def test_a_late_generation_is_an_alert_but_not_an_expiry(self):
        document = healthy()
        document["freshness"]["next_generation_expected_at"] = "2026-08-10T14:00:00Z"
        code, output = run(document)
        self.assertEqual(code, 1)
        self.assertIn("nothing is baking", output)
        self.assertNotIn("went unusable", output)

    def test_a_bitmap_that_disagrees_with_its_shard_list_reaches_nobody(self):
        """Clients refuse such a frame (OBCG §10.3), so the rider silently loses it; the probe is
        the only place that can see it happening to everyone at once."""
        document = healthy()
        document["frames"][0]["present"] = "000000"
        code, output = run(document)
        self.assertEqual(code, 1)
        self.assertIn("presence bitmap flags", output)

    def test_a_planet_with_no_published_shard_is_a_broken_bake(self):
        document = healthy()
        for frame in document["frames"]:
            frame["shards"] = []
            frame["present"] = "000000"
        code, output = run(document)
        self.assertEqual(code, 1)
        self.assertIn("broken bake", output)

    # ── Cost ──────────────────────────────────────────────────────────────────────────────────

    def test_the_set_gate_fires_on_a_dataset_that_grew(self):
        document = healthy()
        code, output = run(document, "--max-set-bytes", str(10 * probe.BYTES_PER_MB))
        self.assertEqual(code, 1)
        self.assertIn("over the 10 MB guard", output)
        self.assertIn("wet global", output, "the alert must carry the measurement it is derived from")

    def test_the_retained_figure_is_reported_but_never_gated(self):
        """`retained = set x (1 + len(previous))` and `len(previous) <= 2`, so a retained gate could
        not fire without the set gate firing first. The figure is the number the bucket actually
        holds, so it is printed; the gate was theatre, so it is gone."""
        document = healthy()
        set_bytes = sum(shard["bytes"] for frame in document["frames"] for shard in frame["shards"])
        code, output = run(document, "--json")
        self.assertEqual(code, 0, output)
        payload = json.loads(output.split("\n\n")[-1])
        self.assertEqual(payload["published_set_bytes"], set_bytes)
        self.assertEqual(payload["retained_bytes"], set_bytes * 3, "current plus two, per OBCG §10.4")

    def test_a_chain_longer_than_the_cap_is_an_alert(self):
        """The one thing about retention a manifest can state wrongly. §10.4's cap is normative on
        readers, and a longer chain means the document and the sweep disagree about what exists."""
        document = healthy()
        document["previous_generations"] = ["20260810T1415Z", "20260810T1400Z", "20260810T1345Z"]
        code, output = run(document)
        self.assertEqual(code, 1)
        self.assertIn("§10.4 caps it at 2", output)

    def test_the_default_is_derived_from_wxr1s_measurement(self):
        """14.69 MB per wet global cycle. The gate is ~2x it, and it is a constant rather than an
        argument so that changing it is a diff someone reviews."""
        self.assertEqual(probe.DEFAULT_MAX_SET_BYTES, 30 * probe.BYTES_PER_MB)
        self.assertEqual(probe.RETAINED_PREVIOUS_GENERATIONS, 2)

    def test_the_retired_byte_gates_still_parse_and_say_they_did_nothing(self):
        """A stale OBC_WX_PROBE_ARGS must not crash the alarm, and must not look obeyed either."""
        args = probe.build_parser().parse_args(["--manifest", "x", "--max-rolling-bytes", "123"])
        self.assertEqual(args.max_retained_bytes, 123)
        code, output = run(healthy(), "--max-retained-bytes", "1")
        self.assertEqual(code, 0, output)
        self.assertIn("--max-retained-bytes is accepted and ignored", output)

    # ── The cadence guard ─────────────────────────────────────────────────────────────────────

    def test_a_timer_firing_faster_than_the_cadence_is_visible_in_one_sample(self):
        """A fast timer re-bakes the same anchor, so the cycle's start walks across the step while
        the generation stands still. Storage never moves; Class A writes do — which is why nothing
        else in this probe can see the mistake."""
        document = healthy()
        document["generated_at"] = "2026-08-10T14:38:00Z"  # 8 min into a 14:30 step
        code, output = run(document, "--now", "2026-08-10T14:39:00Z")
        self.assertEqual(code, 1)
        self.assertIn("CADENCE", output)
        self.assertIn("Class A operations", output)

    def test_the_shipped_timers_jitter_is_not_a_cadence_alert(self):
        """`RandomizedDelaySec=60`: a correct tick is always within a minute of its own boundary,
        and the guard must never fire on one."""
        document = healthy()
        document["generated_at"] = "2026-08-10T14:30:47Z"
        code, output = run(document)
        self.assertEqual(code, 0, output)
        self.assertIn("cycle started 0 min into its 15 min step", output)

    # ── Sources ───────────────────────────────────────────────────────────────────────────────

    def test_expect_sources_catches_a_deploy_that_went_backwards(self):
        document = healthy()
        code, output = run(document, "--expect-sources", "dwd-rv,gfs,mrms")
        self.assertEqual(code, 1)
        self.assertIn("MISSING  mrms", output)
        self.assertIn("this is the binary, not the provider", output)

    def test_every_expected_source_present_is_quiet(self):
        document = healthy()
        listed = ",".join(entry["source_id"] for entry in document["attribution"])
        code, output = run(document, "--expect-sources", listed)
        self.assertEqual(code, 0, output)

    # ── The sweep witness ─────────────────────────────────────────────────────────────────────

    def test_the_swept_generation_is_one_cadence_step_below_the_oldest_kept(self):
        """The arithmetic the sweep witness aims its HEAD with — derived from the document's own
        chain and its own cadence, never from the clock."""
        oldest = probe.parse_generation("20260810T1400Z")
        self.assertEqual(probe.format_generation(oldest), "20260810T1400Z")
        step = probe.timedelta(minutes=15)
        self.assertEqual(probe.format_generation(oldest - step), "20260810T1345Z")

    def test_the_sweep_witness_is_skipped_for_a_local_manifest(self):
        """`--manifest` has no origin to HEAD against, and a probe that invented one would alarm on
        a document a maintainer is reading by hand."""
        code, output = run(healthy())
        self.assertEqual(code, 0, output)
        self.assertNotIn("SWEEP", output)

    def test_the_sweep_witness_needs_a_full_retention_chain(self):
        document = healthy()
        document["previous_generations"] = ["20260810T1415Z"]
        report, alerts, result = [], [], {}
        probe.check_sweep(document, "https://example.invalid", 0.01, report, alerts, result)
        self.assertEqual(alerts, [])
        self.assertIn("nothing has fallen off the chain yet", report[0])

    def test_a_swept_generation_that_is_still_fetchable_is_the_alarm(self):
        """The branch the whole check exists for. A 200 on a generation the manifest stopped naming
        means the sweep is not collecting — and nothing computed from the manifest can see that."""
        document = healthy()
        with fake_head(lambda url: (b"", "")):
            report, alerts, result = [], [], {}
            probe.check_sweep(document, "https://wx.example", 1.0, report, alerts, result)
        self.assertEqual(len(alerts), 1, report)
        self.assertIn("retention sweep is not collecting", alerts[0])
        self.assertIn("SWEEP", report[0])

    def test_a_404_on_the_swept_generation_is_the_healthy_answer(self):
        document = healthy()
        report, alerts, result = [], [], {}
        with http_status(404) as gone, fake_head(gone):
            probe.check_sweep(document, "https://wx.example", 1.0, report, alerts, result)
        self.assertEqual(alerts, [])
        self.assertIn("ok       swept: 20260810T1345Z is gone", report[0])

    def test_a_non_404_http_error_is_inconclusive_rather_than_an_alarm(self):
        """What a misconfigured public-access setting looks like. The probe must not report a
        sweep failure it cannot actually see."""
        document = healthy()
        report, alerts, result = [], [], {}
        with http_status(500) as broken, fake_head(broken):
            probe.check_sweep(document, "https://wx.example", 1.0, report, alerts, result)
        self.assertEqual(alerts, [])
        self.assertIn("inconclusive: HTTP 500", report[0])

    def test_the_sweep_witness_probes_the_shard_that_is_always_published(self):
        """Shard row 0 reaches below `covered_rows.start`, so it always holds no-data cells, and a
        shard with one no-data cell is never omitted as dry (OBCG §10.3). That is what makes a 404
        there mean "swept" rather than "was dry"."""
        document = healthy()
        self.assertGreater(document["lattice"]["covered_rows"]["start"], 0)
        report, alerts, result = [], [], {}
        # An unresolvable host makes the HEAD fail; the point is the key it aimed at.
        probe.check_sweep(document, "https://wx.invalid", 0.01, report, alerts, result)
        self.assertEqual(result["sweep_probe_key"], "wx/v2/20260810T1345Z/f0/s0-0.obcg")
        self.assertEqual(alerts, [], "an unreachable HEAD is inconclusive, never an alert")

if __name__ == "__main__":
    unittest.main()
