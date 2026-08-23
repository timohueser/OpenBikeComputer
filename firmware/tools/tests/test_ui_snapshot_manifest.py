import hashlib
import importlib.util
import re
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "ui_snapshot_manifest.py"
SPEC = importlib.util.spec_from_file_location("ui_snapshot_manifest", SCRIPT)
manifest_tool = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = manifest_tool
SPEC.loader.exec_module(manifest_tool)


def digest(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


class UiSnapshotManifestTests(unittest.TestCase):
    """The four rejections the manifest exists for — a changed frame, a frame the sweep stopped
    producing, one it started producing, and a manifest that names a frame twice — plus the round
    trip that makes `update` a usable answer to any of them."""

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
        return manifest_tool.main(["check", str(self.manifest), str(self.out)])

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
            manifest_tool.read_manifest(self.manifest)
        self.assertEqual(self.check(), 2)  # …and the command reports it rather than crashing

    def test_a_malformed_manifest_is_rejected(self):
        for bad in ("not-a-digest  home.png\n", "abc\n", f"{digest(b'x')}  sub/home.png\n"):
            self.manifest.write_text(bad)
            with self.assertRaises(manifest_tool.ManifestError):
                manifest_tool.read_manifest(self.manifest)

    def test_an_empty_sweep_is_rejected_rather_than_recorded(self):
        """A sweep that rendered nothing must not quietly become an empty manifest — that would
        turn a broken simulator into a passing check on the next run."""
        for frame in self.out.iterdir():
            frame.unlink()
        self.assertEqual(self.check(), 2)
        self.assertEqual(manifest_tool.main(["update", str(self.manifest), str(self.out)]), 2)
        self.assertNotEqual(self.manifest.read_text(), "")

    def test_update_records_the_sweep_and_check_then_passes(self):
        (self.out / "map.png").write_bytes(b"map-pixels-reworked")
        (self.out / "climb.png").write_bytes(b"a new screen")
        (self.out / "home.png").unlink()
        self.assertEqual(manifest_tool.main(["update", str(self.manifest), str(self.out)]), 0)
        self.assertEqual(self.check(), 0)
        rows = manifest_tool.read_manifest(self.manifest)
        self.assertEqual(sorted(rows), ["climb.png", "map.png"])

    def test_the_manifest_is_sorted_by_basename(self):
        for name in ("zebra.png", "alpha.png"):
            (self.out / name).write_bytes(name.encode())
        manifest_tool.main(["update", str(self.manifest), str(self.out)])
        names = [line.split()[1] for line in self.manifest.read_text().splitlines()]
        self.assertEqual(names, sorted(names))


class CommittedManifestTests(unittest.TestCase):
    """The committed manifest is itself parseable, and lines up with the sweep script one-for-one —
    the two claims the sweep cannot make about itself, because it only ever sees the frames it just
    rendered."""

    REPO = Path(__file__).parents[3]
    LANGUAGES = ("de", "fr", "es")

    def sweep_source(self) -> str:
        return (self.REPO / "firmware" / "ui-snapshots.sh").read_text()

    def committed_rows(self) -> dict[str, str]:
        return manifest_tool.read_manifest(self.REPO / "firmware" / "ui-snapshots.sha256")

    def test_the_committed_manifest_parses(self):
        rows = self.committed_rows()
        self.assertTrue(rows)
        self.assertTrue(all(name.endswith(".png") for name in rows))

    def test_every_render_command_states_its_expected_screen(self):
        commands = [line for line in self.sweep_source().splitlines() if '--png "$OUT/' in line]
        without = [line for line in commands if "--expect-screen " not in line]
        self.assertEqual(without, [], "every render command must state --expect-screen")

    def test_the_manifest_names_exactly_what_the_sweep_renders(self):
        """A `check` run only compares the manifest to the frames a sweep *just produced*, so a
        command deleted together with its row passes silently and the screen leaves the net unnoticed.
        This reads the intended frame set out of the script instead — the `--png` targets, with the
        per-language loop's `$lang` expanded — so a row with no command and a command with no row are
        both named. Derived rather than a hand-kept count: the literal that used to live at the foot
        of the sweep drifted 37 frames behind the script before it was deleted."""
        rendered = set()
        for target in re.findall(r'--png "\$OUT/([^"]+\.png)"', self.sweep_source()):
            if "$lang" in target:
                rendered.update(target.replace("$lang", language) for language in self.LANGUAGES)
            else:
                rendered.add(target)
        recorded = set(self.committed_rows())
        self.assertEqual(sorted(rendered - recorded), [], "the sweep renders frames the manifest does not name")
        self.assertEqual(sorted(recorded - rendered), [], "the manifest names frames the sweep does not render")
