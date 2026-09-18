import hashlib
import re
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from ui_frames import manifest as manifest_tool  # noqa: E402
from ui_frames import table as table_tool  # noqa: E402

REPO = Path(__file__).parents[3]
TABLE = REPO / "firmware" / "ui-frames.toml"
MANIFEST = REPO / "firmware" / "ui-snapshots.sha256"


def digest(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


class TableTests(unittest.TestCase):
    """The three constructs the table holds, and the two ways a row can name nothing."""

    def load(self, body: str):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "ui-frames.toml"
            path.write_text(body)
            return table_tool.load(path, fixtures="/fx", repo="/repo")

    def test_a_fragment_expands_and_can_use_another_fragment(self):
        (frame,) = self.load(
            '[script]\npeak = "B d d d p f"\narticle = "{peak} p f"\n'
            '[[frame]]\nname = "peak"\nscript = "{article} u"\nexpect = "PeakView"\n'
        )
        self.assertEqual(frame.script, "B d d d p f p f u")

    def test_a_repetition_expands_to_that_many_tokens(self):
        (frame,) = self.load('[[frame]]\nname = "wait"\nscript = "p w*7 b"\nexpect = "RideDetail"\n')
        self.assertEqual(frame.script, "p w w w w w w w b")

    def test_langs_gives_one_frame_for_each_language(self):
        frames = self.load('[[frame]]\nname = "menu"\nexpect = "Menu"\nlangs = ["de", "fr"]\n')
        self.assertEqual([frame.name for frame in frames], ["menu-de", "menu-fr"])
        self.assertEqual([frame.lang for frame in frames], ["de", "fr"])

    def test_a_path_expands_from_the_roots(self):
        (frame,) = self.load(
            '[paths]\nmap = "{fixtures}/sim-grimsel/grimsel.obcm"\n'
            '[[frame]]\nname = "home"\nmap = "{map}"\nexpect = "Home"\n'
        )
        self.assertEqual(frame.map, "/fx/sim-grimsel/grimsel.obcm")

    def test_an_unknown_fragment_names_itself(self):
        with self.assertRaisesRegex(table_tool.TableError, "unknown script fragment `nope`"):
            self.load('[[frame]]\nname = "home"\nscript = "{nope}"\nexpect = "Home"\n')

    def test_a_row_without_a_destination_screen_is_rejected(self):
        """A recipe that walks a menu is a hostage to that menu's station order, so `expect` is not
        optional: without it a new station silently snapshots another screen under the old name."""
        with self.assertRaises(table_tool.TableError):
            self.load('[[frame]]\nname = "home"\nscript = "p"\n')

    def test_two_frames_cannot_share_a_name(self):
        """Names are the only thing tying a frame to the row that made it. A repeat would be
        invisible: the second render overwrites the first and one digest row still matches."""
        with self.assertRaisesRegex(table_tool.TableError, "two frames are named home"):
            self.load('[[frame]]\nname = "home"\nexpect = "Home"\n[[frame]]\nname = "home"\nexpect = "Home"\n')


class ManifestTests(unittest.TestCase):
    """The five rejections the manifest exists for — a changed frame, a frame the sweep stopped
    producing, one it started producing, a manifest that names a frame twice, and two frames with
    different names over identical pixels — plus the round trip that makes `--accept` a usable
    answer to any of them."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.out = self.root / "sweep"
        self.out.mkdir()
        self.manifest = self.root / "ui-snapshots.sha256"
        self.frames = {"home.png": b"home-pixels", "map.png": b"map-pixels"}
        for name, payload in self.frames.items():
            (self.out / name).write_bytes(payload)
        self.manifest.write_text(
            "".join(f"{digest(self.frames[name])}  {name}\n" for name in sorted(self.frames))
        )
        self.addCleanup(self._tmp.cleanup)

    def check(self):
        return manifest_tool.check(self.manifest, self.out)

    def test_a_clean_sweep_passes(self):
        self.assertEqual(self.check(), 0)

    def test_a_changed_frame_is_rejected(self):
        (self.out / "map.png").write_bytes(b"map-pixels-but-one-pixel-moved")
        self.assertEqual(self.check(), 1)

    def test_a_missing_frame_is_rejected(self):
        (self.out / "map.png").unlink()
        self.assertEqual(self.check(), 1)

    def test_an_extra_frame_is_rejected(self):
        (self.out / "climb.png").write_bytes(b"a screen nobody recorded")
        self.assertEqual(self.check(), 1)

    def test_a_duplicate_basename_is_rejected(self):
        row = f"{digest(self.frames['home.png'])}  home.png\n"
        self.manifest.write_text(self.manifest.read_text() + row)
        with self.assertRaisesRegex(manifest_tool.ManifestError, "duplicate entry for home.png"):
            manifest_tool.read(self.manifest)

    def test_two_frames_with_one_image_are_rejected(self):
        """The pixels are the only witness: a wrong-state frame under the right name passes
        `expect` and every digest check."""
        (self.out / "map-panning.png").write_bytes(self.frames["map.png"])
        manifest_tool.accept(self.manifest, self.out)
        self.assertEqual(self.check(), 1)

    def test_a_declared_identical_pair_passes(self):
        """Declaring the pair turns the identity from an accident into the claim."""
        (self.out / "map-panning.png").write_bytes(self.frames["map.png"])
        manifest_tool.accept(self.manifest, self.out)
        original = list(manifest_tool.IDENTICAL_BY_DESIGN)
        manifest_tool.IDENTICAL_BY_DESIGN.append({"map.png", "map-panning.png"})
        self.addCleanup(lambda: manifest_tool.IDENTICAL_BY_DESIGN.__setitem__(slice(None), original))
        self.assertEqual(self.check(), 0)

    def test_a_malformed_manifest_is_rejected(self):
        for bad in ("not-a-digest  home.png\n", "abc\n", f"{digest(b'x')}  sub/home.png\n"):
            self.manifest.write_text(bad)
            with self.assertRaises(manifest_tool.ManifestError):
                manifest_tool.read(self.manifest)

    def test_an_empty_sweep_is_rejected_rather_than_recorded(self):
        """A sweep that rendered nothing must not quietly become an empty manifest — that would
        turn a broken simulator into a passing check on the next run."""
        for frame in self.out.iterdir():
            frame.unlink()
        for call in (manifest_tool.check, manifest_tool.accept):
            with self.assertRaises(manifest_tool.ManifestError):
                call(self.manifest, self.out)
        self.assertNotEqual(self.manifest.read_text(), "")

    def test_accept_records_the_sweep_and_check_then_passes(self):
        (self.out / "map.png").write_bytes(b"map-pixels-reworked")
        (self.out / "climb.png").write_bytes(b"a new screen")
        (self.out / "home.png").unlink()
        self.assertEqual(manifest_tool.accept(self.manifest, self.out), 0)
        self.assertEqual(self.check(), 0)
        self.assertEqual(sorted(manifest_tool.read(self.manifest)), ["climb.png", "map.png"])

    def test_accept_with_names_moves_only_those_rows(self):
        (self.out / "map.png").write_bytes(b"map-pixels-reworked")
        (self.out / "home.png").write_bytes(b"home-pixels-reworked")
        manifest_tool.accept(self.manifest, self.out, ["map"])
        rows = manifest_tool.read(self.manifest)
        self.assertEqual(rows["map.png"], digest(b"map-pixels-reworked"))
        self.assertEqual(rows["home.png"], digest(b"home-pixels"))

    def test_the_manifest_is_sorted_by_basename(self):
        for name in ("zebra.png", "alpha.png"):
            (self.out / name).write_bytes(name.encode())
        manifest_tool.accept(self.manifest, self.out)
        names = [line.split()[1] for line in self.manifest.read_text().splitlines()]
        self.assertEqual(names, sorted(names))


class CommittedTableTests(unittest.TestCase):
    """The committed table and manifest are parseable and line up one-for-one — the two claims a
    sweep cannot make about itself, because it only ever sees the frames it just rendered."""

    #: The one screen with no frame, and why. Its only entry is
    #: `App::offer_recovered_ride(RideContinuation)` — a host call carrying thirteen reconstructed
    #: accumulator fields that no gesture stands in for, and which the simulator has no seed for.
    #: Adding that seed is how this set empties; widening it is how the net quietly stops meaning
    #: anything, so a new name here needs the same argument.
    UNCOVERED_SCREENS = {"RideRecovery"}

    def frames(self):
        return table_tool.load(TABLE)

    def test_no_frame_and_no_digest_row_is_left_over(self):
        """A `--check` run only compares the manifest to the frames a sweep *just produced*, so a
        row deleted together with its frame passes silently and the screen leaves the net unnoticed.
        This reads the intended frame set out of the table instead, so a row with no frame and a
        frame with no row are both named."""
        self.assertEqual(manifest_tool.stale([frame.name for frame in self.frames()], MANIFEST), [])

    def test_every_screen_but_the_documented_exception_has_a_frame(self):
        """The net's actual claim, enforced against the app's own screen table rather than against
        the manifest — so it cannot be satisfied by deleting a frame and its row together.

        This is what a `screens!` row is *for* here: add a screen and the table must gain a frame
        naming it, or this fails. The reverse is covered too — an `expect` naming a screen the table
        no longer has is a row pinned to a variant that cannot be reached."""
        source = (REPO / "firmware" / "obc-app" / "src" / "screen" / "mod.rs").read_text()
        body = re.search(r"^screens! \{$(.*?)^\}$", source, re.MULTILINE | re.DOTALL)
        self.assertIsNotNone(body, "the screens! table is not where this test expects it")
        declared = set(re.findall(r"^    (\w+)\(\w+\) => Caps", body.group(1), re.MULTILINE))
        self.assertGreater(len(declared), 50, "the screens! table parsed suspiciously small")

        expected = {frame.expect for frame in self.frames()}
        self.assertEqual(
            sorted(declared - expected - self.UNCOVERED_SCREENS),
            [],
            "a screen in the screens! table has no frame in the table",
        )
        self.assertEqual(sorted(expected - declared), [], "a frame expects a screen the table does not declare")
        self.assertEqual(
            sorted(self.UNCOVERED_SCREENS & expected),
            [],
            "a screen listed as uncovered now has a frame — remove it from UNCOVERED_SCREENS",
        )
