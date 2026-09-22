"""`obc r2 rm`, with rclone replaced by a recorder.

rclone is not a test dependency, so what the tests hold is what this tool owns: what the
plan says, what lets it run, what it refuses to touch, and the order the calls go out in.
The catalogue the guard reads is the shipped example document, not a hand-made one, because
the guard's whole job is to match the shape the publisher really writes. No test reaches the
network.
"""

import importlib.util
import io
import json
import os
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from urllib.parse import urlsplit


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tools" / "r2.py"
SPEC = importlib.util.spec_from_file_location("r2", MODULE_PATH)
r2 = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = r2
SPEC.loader.exec_module(r2)


SCHEMA = ROOT / "host" / "obc-pack" / "schema"
CATALOG = json.loads((SCHEMA / "catalog.example.json").read_text(encoding="utf-8"))
CELL_INDEX = json.loads((SCHEMA / "cell-index.example.json").read_text(encoding="utf-8"))

#: The example document publishes under this base, so the prefix inside the bucket is
#: `catalog` and every url it names is `<BASE>/<key under that prefix>`.
BASE = "https://maps.example.org/catalog"
PREFIX = "catalog"


def key_of(url: str) -> str:
    return f"{PREFIX}{urlsplit(url).path[len(urlsplit(BASE).path):]}"


BAND_INDEX = key_of(next(row["url"] for row in CATALOG["cell_index"] if row["band"] == "fine"))
CELL = key_of(CELL_INDEX["cells"][0]["url"])
TILE = "reference/v1/16/3410/2882.tif"
ARCHIVE_INDEX = "reference/v1/index.json"
STRAY = "uploads/scratch.obcm"


def published(document) -> dict[str, int]:
    """Every object the example document names, as the fake bucket holds it."""

    found, stack = {}, [document]
    while stack:
        item = stack.pop()
        if isinstance(item, str) and item.startswith(f"{BASE}/"):
            found[key_of(item)] = 1000
        elif isinstance(item, dict):
            stack.extend(item.values())
        elif isinstance(item, list):
            stack.extend(item)
    return found


#: What the fake bucket holds, by key: everything the example catalogue names, the
#: reference archive, and a stray upload nothing points at.
BUCKET = {
    **published(CATALOG),
    **published(CELL_INDEX),
    f"{PREFIX}/catalog.json": 8100,
    f"{PREFIX}/LICENSE.txt": 1100,
    f"{PREFIX}/regions/europe.json": 640,
    TILE: 2097152,
    ARCHIVE_INDEX: 900,
    STRAY: 17,
    "uploads/older.obcm": 21,
}

INDEX = {
    "schema": 1, "step_log2": 6, "tile_log2": 16,
    "sources": {"ch": {"product": "swissALTI3D"}, "es": {"product": "MDT05"}},
    "tiles": {"3410/2882": "ch", "0100/0200": "es"},
    "contributors": {"3410/2882": ["ch"], "0100/0200": ["es"]},
    "sha256": {"3410/2882": "aa" * 32, "0100/0200": "bb" * 32},
}


class Rm(unittest.TestCase):
    def setUp(self):
        self.calls, self.sent = [], {}
        self.bucket = dict(BUCKET)
        self.log = None
        for name, value in (
            ("OBC_R2_ACCOUNT_ID", "acc"), ("OBC_R2_BUCKET", "maps"), ("OBC_R2_PREFIX", PREFIX),
            ("OBC_R2_ACCESS_KEY_ID", "key"), ("OBC_R2_SECRET_ACCESS_KEY", "s3cret"),
            ("OBC_MAPS_BASE_URL", BASE), ("OBC_CATALOG_URL", f"{BASE}/catalog.json"),
        ):
            previous = os.environ.get(name)
            os.environ[name] = value
            self.addCleanup(lambda n=name, p=previous: os.environ.__setitem__(n, p) if p
                            else os.environ.pop(n, None))
        real = r2.run_rclone
        r2.run_rclone = self.record
        self.addCleanup(lambda: setattr(r2, "run_rclone", real))

    def body(self, key):
        """What the fake bucket hands back for one key."""

        if key == ARCHIVE_INDEX:
            return json.dumps(INDEX)
        if key == f"{PREFIX}/catalog.json":
            return json.dumps(CATALOG)
        if r2.BAND_INDEX.search(key):
            return json.dumps(CELL_INDEX if key == BAND_INDEX else {"cells": []})
        if key == r2.REMOVAL_LOG:
            return self.log
        return "{}"

    def record(self, argv, env, capture=False):
        self.calls.append((argv, env))
        place = argv[1].removeprefix("OBCR2:maps").strip("/") if len(argv) > 1 else ""
        if argv[0] == "lsf":  # does the bucket hold this one object?
            name = argv[argv.index("--include") + 1].lstrip("/")
            key = f"{place}/{name}" if place else name
            held = key in self.bucket or (key == r2.REMOVAL_LOG and self.log is not None)
            return f"{name}\n" if held else ""
        if argv[0] == "lsjson" and "--files-from" in argv:
            asked = Path(argv[argv.index("--files-from") + 1]).read_text(encoding="utf-8").split()
            return json.dumps([{"Path": key, "Size": self.bucket[key], "ModTime": "2026-09-01T10:11:12Z"}
                               for key in asked if key in self.bucket])
        if argv[0] == "lsjson":
            head = f"{place}/"
            return json.dumps([{"Path": key[len(head):], "Size": size, "ModTime": "2026-09-01T10:11:12Z"}
                               for key, size in sorted(self.bucket.items()) if key.startswith(head)])
        if argv[0] == "copyto" and argv[1].startswith("OBCR2:"):  # a read out of the bucket
            Path(argv[2]).write_text(self.body(place), encoding="utf-8")
        if argv[0] == "copyto" and argv[2].startswith("OBCR2:"):  # a write back into it
            self.sent[argv[2].removeprefix("OBCR2:maps/")] = Path(argv[1]).read_text(encoding="utf-8")
        if argv[0] == "deletefile":
            self.bucket.pop(place, None)
        return ""

    def rm(self, *args):
        self.printed = io.StringIO()
        with redirect_stdout(self.printed):
            return r2.main(["rm", *args])

    def read_keys(self):
        return [argv[1].removeprefix("OBCR2:maps/") for argv, _ in self.calls
                if argv[0] == "copyto" and argv[1].startswith("OBCR2:")]

    def deleted(self):
        return [argv[1].removeprefix("OBCR2:maps/") for argv, _ in self.calls if argv[0] == "deletefile"]

    def apply(self, key, *args, reason="a bad ingest"):
        return self.rm(key, *args, "--apply", "--reason", reason,
                       "--confirm", r2.confirmation([key]))

    # ── the plan ────────────────────────────────────────────────────────────

    def test_the_plan_states_what_the_live_listing_holds(self):
        self.assertEqual(self.rm(STRAY), 0)
        printed = self.printed.getvalue()
        self.assertIn(STRAY, printed)
        self.assertIn("17", printed)                     # the size, from the listing
        self.assertIn("2026-09-01T10:11:12Z", printed)   # and the last-modified
        self.assertIn(f'--confirm "{r2.confirmation([STRAY])}"', printed)
        self.assertEqual(self.deleted(), [])

    def test_a_key_the_bucket_does_not_hold_is_an_error(self):
        self.assertEqual(self.rm(STRAY, "uploads/gone.obcm"), 1)
        self.assertEqual(self.deleted(), [])

    def test_the_bucket_root_is_never_a_target(self):
        for prefix in ("", "/", ".", "../catalog"):
            with self.subTest(prefix=prefix):
                self.assertEqual(self.rm("--prefix", prefix), 1)
        self.assertEqual(self.rm(STRAY, "--prefix", PREFIX), 1)  # one form, never both
        self.assertEqual(self.deleted(), [])

    def test_a_prefix_wider_than_one_call_takes_is_refused(self):
        self.bucket = {f"uploads/{n:04d}.obcm": 10 for n in range(r2.RM_CAP + 1)}
        self.assertEqual(self.rm("--prefix", "uploads"), 1)
        self.assertNotIn("--files-from", [word for argv, _ in self.calls for word in argv])
        self.assertEqual(self.deleted(), [])

    def test_a_prefix_inside_the_cap_plans_every_object_under_it(self):
        self.assertEqual(self.rm("--prefix", "uploads"), 0)
        printed = self.printed.getvalue()
        self.assertIn(STRAY, printed)
        self.assertIn("uploads/older.obcm", printed)
        self.assertEqual(self.deleted(), [])

    # ── what the catalogue protects ─────────────────────────────────────────

    def test_the_catalogue_root_the_log_and_the_archive_index_refuse(self):
        self.bucket[r2.REMOVAL_LOG] = 40
        self.log = "{}\n"
        for key, word in ((f"{PREFIX}/catalog.json", "catalogue root"),
                          (f"{PREFIX}/LICENSE.txt", "catalogue root"),
                          (f"{PREFIX}/regions/europe.json", "catalogue root"),
                          (r2.REMOVAL_LOG, "removal history"),
                          (ARCHIVE_INDEX, "reference archive index")):
            with self.subTest(key=key):
                self.assertEqual(self.rm(key), 1)
                self.assertIn(word, self.printed.getvalue())
                self.assertEqual(self.deleted(), [])

    def test_an_object_the_root_names_by_absolute_url_is_protected(self):
        """The real root names its band indexes by URL, never by bucket key."""

        self.assertEqual(self.rm(BAND_INDEX), 1)
        self.assertIn("live catalogue names it", self.printed.getvalue())
        self.assertEqual(self.deleted(), [])

    def test_a_live_cell_is_protected_through_its_band_index(self):
        """A cell is named one level down from the root, so the guard walks there."""

        self.assertEqual(self.rm(CELL), 1)
        self.assertIn("live catalogue names it", self.printed.getvalue())
        self.assertIn(BAND_INDEX, self.read_keys())

        self.assertEqual(self.apply(CELL, "--i-mean-it", CELL), 0)
        self.assertEqual(self.deleted(), [CELL])
        self.assertIn(r2.REPUBLISH, self.printed.getvalue())

    def test_with_no_base_url_in_the_environment_the_url_path_still_protects(self):
        for name in ("OBC_MAPS_BASE_URL", "OBC_CATALOG_URL"):
            os.environ.pop(name)
        self.assertIsNone(r2.maps_base())
        self.assertEqual(self.rm(CELL), 1)
        self.assertIn("live catalogue names it", self.printed.getvalue())

    def test_a_base_url_the_catalogue_does_not_use_still_protects(self):
        """A stale or mis-typed base may protect more than it must, never less."""

        os.environ["OBC_MAPS_BASE_URL"] = "https://maps.example.org/somewhere-else"
        self.assertEqual(self.rm(CELL), 1)
        self.assertIn("live catalogue names it", self.printed.getvalue())
        self.assertEqual(self.deleted(), [])

    def test_protection_fails_closed_when_the_catalogue_cannot_be_read(self):
        del self.bucket[f"{PREFIX}/catalog.json"]
        self.assertEqual(self.rm(CELL), 1)
        self.assertEqual(self.deleted(), [])

        self.bucket[f"{PREFIX}/catalog.json"] = 8100
        self.body = lambda key: "{ not json"
        self.assertEqual(self.rm(CELL), 1)
        self.assertEqual(self.deleted(), [])

    def test_protection_fails_closed_when_a_band_index_is_gone(self):
        del self.bucket[BAND_INDEX]
        self.assertEqual(self.rm(CELL), 1)
        self.assertEqual(self.deleted(), [])

    def test_no_catalog_deletes_only_what_is_named_by_hand(self):
        del self.bucket[f"{PREFIX}/catalog.json"]
        self.assertEqual(self.rm(CELL, "--no-catalog"), 1)
        self.assertEqual(self.deleted(), [])
        self.assertEqual(self.apply(CELL, "--no-catalog", "--i-mean-it", CELL), 0)
        self.assertEqual(self.deleted(), [CELL])

    def test_i_mean_it_has_to_name_a_key_this_plan_deletes(self):
        self.assertEqual(self.rm(STRAY, "--i-mean-it", f"{PREFIX}/catalog.json"), 1)
        self.assertEqual(self.deleted(), [])

    # ── the confirmation ────────────────────────────────────────────────────

    def test_apply_needs_the_plans_own_confirmation_and_a_reason(self):
        for extra in ([], ["--confirm", r2.confirmation([STRAY])],
                      ["--confirm", "1-0000000000", "--reason", "x"],
                      ["--reason", "x", "--confirm", r2.confirmation([CELL])]):
            with self.subTest(extra=extra):
                self.assertEqual(self.rm(STRAY, "--apply", *extra), 1)
                self.assertEqual(self.deleted(), [])

    def test_a_swap_between_the_plan_and_the_apply_is_refused(self):
        """Same count and same first key, one object exchanged: the digest still sees it."""

        self.assertEqual(self.rm("--prefix", "uploads"), 0)
        reviewed = self.printed.getvalue().rsplit('--confirm "', 1)[1].split('"')[0]
        self.bucket["uploads/zz-new.obcm"] = 9
        del self.bucket["uploads/older.obcm"]
        self.assertEqual(self.rm("--prefix", "uploads", "--apply", "--reason", "x",
                                 "--confirm", reviewed), 1)
        self.assertEqual(self.deleted(), [])

    # ── what an apply does, and in which order ──────────────────────────────

    def test_the_log_goes_first_then_the_index_then_the_objects(self):
        self.assertEqual(self.apply(TILE), 0)
        writes = [n for n, (argv, _) in enumerate(self.calls)
                  if argv[0] == "copyto" and argv[2].startswith("OBCR2:")]
        deletes = [n for n, (argv, _) in enumerate(self.calls) if argv[0] == "deletefile"]
        self.assertEqual([self.calls[n][0][2].removeprefix("OBCR2:maps/") for n in writes],
                         [r2.REMOVAL_LOG, ARCHIVE_INDEX])
        self.assertLess(max(writes), min(deletes))

        index = json.loads(self.sent[ARCHIVE_INDEX])
        self.assertEqual(index["tiles"], {"0100/0200": "es"})
        self.assertNotIn("3410/2882", index["sha256"])
        self.assertEqual(sorted(index["sources"]), ["es"])  # `ch` holds no other tile
        self.assertEqual(self.deleted(), [TILE])

    def test_the_log_keeps_who_when_what_and_why(self):
        self.log = '{"key": "uploads/old.obcm"}\n'
        self.assertEqual(self.apply(STRAY, reason="a stray upload"), 0)
        kept, added = self.sent[r2.REMOVAL_LOG].splitlines()
        self.assertEqual(json.loads(kept)["key"], "uploads/old.obcm")  # the history is appended to
        record = json.loads(added)
        self.assertEqual(record["key"], STRAY)
        self.assertEqual(record["bytes"], 17)
        self.assertEqual(record["reason"], "a stray upload")
        self.assertTrue(record["removed"].endswith("Z") and record["by"])

    def test_the_secret_is_in_the_environment_only(self):
        self.rm(STRAY)
        argv, env = self.calls[0]
        self.assertEqual(env["RCLONE_CONFIG_OBCR2_SECRET_ACCESS_KEY"], "s3cret")
        self.assertEqual(env["RCLONE_CONFIG_OBCR2_ENDPOINT"], "https://acc.r2.cloudflarestorage.com")
        self.assertNotIn("s3cret", " ".join(argv))

    def test_the_archive_sits_where_the_ingest_tool_publishes_it(self):
        """One definition of the bucket, so a delete cannot land beside a publish."""

        self.assertEqual(r2.archive_prefix(), "reference/v1")
        self.assertEqual(r2.bucket_remote().path, "OBCR2:maps")
        self.assertEqual(r2.reference_tile(TILE), "3410/2882")
        self.assertIsNone(r2.reference_tile(CELL))


if __name__ == "__main__":
    unittest.main()
