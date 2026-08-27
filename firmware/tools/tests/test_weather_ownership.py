"""Board-composition contracts for the weather ownership cutover (#1549).

The nRF crate is Thumb-only, so the three facts that make `WeatherDomain` live on a real device are
pinned here rather than by a host test that cannot run the ride loop: the installed-data report
inside the catalog read, the due plane's in-flight level reported beside the link level, and the
resample revision reported when the host's snapshot actually moves.

The fourth check is the one that has no natural home anywhere else: **a refresh flag must not cross
a render signature again**. Two sources of truth for "an update is running" is what this slice
deleted, and the shape it came back in would be a `refreshing:` argument beside the snapshot.
"""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[3]
RIDE = (ROOT / "firmware/obc-fw-nrf54l/src/ride.rs").read_text()
APP = (ROOT / "firmware/obc-app/src/app.rs").read_text()
SCREEN = (ROOT / "firmware/obc-app/src/screen/mod.rs").read_text()
MENU = (ROOT / "firmware/obc-app/src/screen/menu.rs").read_text()


def body(source: str, start: str, end: str) -> str:
    """Return one deliberately delimited production section."""
    first = source.index(start)
    return source[first : source.index(end, first)]


class WeatherOwnershipTests(unittest.TestCase):
    def test_the_catalog_read_reports_the_installed_weather_identity(self) -> None:
        """The domain's first production writer. Without this, `installed()` is `None` on glass."""
        rescan = body(RIDE, "fn read_catalogs", "/// A `no_std`")
        self.assertIn("facts.note_weather_data(", rescan)
        self.assertIn("DataIdentity::new(weather.id.0)", rescan)
        self.assertIn("Revision::new(weather.revision.0)", rescan)
        self.assertLess(
            rescan.index("active_weather(flat)"),
            rescan.index("facts.note_weather_data("),
            "the reported identity must be the head the read just selected",
        )

    def test_the_due_planes_level_and_the_resample_are_reported_as_facts(self) -> None:
        """Both weather levels the loop owns, and neither of them read at render time."""
        self.assertIn("exec.facts.note_weather_refreshing(crate::ble::weather_refreshing());", RIDE)
        self.assertIn("exec.facts.note_weather_sample(weather_sample);", RIDE)
        self.assertLess(
            RIDE.index("exec.facts.note_weather_refreshing"),
            RIDE.index("app.run_pass("),
            "a level is reported ahead of the pass that consumes it at stage 2",
        )

    def test_the_board_no_longer_decides_when_a_refresh_is_worth_a_radio_trip(self) -> None:
        """The screen sniff is gone; the executor serves an effect the domain decided."""
        self.assertNotIn("was_on_weather", RIDE)
        self.assertNotIn("weather_refresh_in_flight", RIDE)
        served = body(RIDE, "WeatherEffect::RequestRefresh { token }", "Every effect this frame carried")
        self.assertIn("crate::ble::request_weather_now();", served)
        self.assertIn("WeatherOutcome::Raised { token }", served)

    def test_the_refresh_intent_has_exactly_one_producer(self) -> None:
        """`menu.rs`'s row is the only push site of the dashboard, so it is the only entry edge.

        Naming the intent on the Weather screen's own `handle` instead would make Back from Hourly
        manufacture a second urgent request, which is the bug the board's deleted `was_on_weather`
        comparison existed to avoid.
        """
        pushes = sorted(
            path.name
            for path in (ROOT / "firmware/obc-app/src").rglob("*.rs")
            if "Transition::Push(Screen::Weather(" in path.read_text()
        )
        self.assertEqual(pushes, ["menu.rs"])
        self.assertIn("WeatherIntent::RefreshRequested", MENU)
        self.assertEqual(MENU.count("WeatherIntent::RefreshRequested"), 1)

    def test_no_refreshing_flag_crosses_a_render_signature(self) -> None:
        """The cue is the domain's answer, filled where the render frame is built and nowhere else.

        `WeatherFeed` paired a snapshot borrow with a `refreshing: bool` and every host filled the
        bool from its own platform flag, so the cue and `WeatherDomain::refreshing()` could disagree.
        Exactly two places may declare that flag now: the domain's own aggregate value, and the
        render key that names what the weather pages draw. A third is a second source of truth.
        """
        owners = {
            "firmware/obc-app/src/weather.rs",  # WeatherVisible — the domain's own answer
            "firmware/obc-app/src/render_key.rs",  # WeatherKey — what the pages declare they draw
        }
        pattern = re.compile(r"\brefreshing\s*:\s*bool\b")
        offenders = []
        for path in sorted((ROOT / "firmware").rglob("*.rs")) + sorted((ROOT / "apps").rglob("*.rs")):
            if "target" in path.parts or str(path.relative_to(ROOT)) in owners:
                continue
            for number, line in enumerate(path.read_text().splitlines(), 1):
                if pattern.search(line):
                    offenders.append(f"{path.relative_to(ROOT)}:{number}: {line.strip()}")
        self.assertEqual(offenders, [], "a second answer to 'is an update running' has grown back")
        self.assertIn("let weather_refreshing = self.weather.refreshing();", APP)


if __name__ == "__main__":
    unittest.main()
