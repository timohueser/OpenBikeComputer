"""`obc r2 rm`, with the R2 client replaced by a recorder.

The client has its own tests in host/obc-data, so what these tests hold is what this tool
owns: what the plan says, what lets it run, what it refuses to touch, and the order the
calls go out in.
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
        for name, value in (
            ("OBC_R2_ACCOUNT_ID", "acc"), ("OBC_R2_BUCKET", "maps"), ("OBC_R2_PREFIX", PREFIX),
            ("OBC_R2_ACCESS_KEY_ID", "key"), ("OBC_R2_SECRET_ACCESS_KEY", "s3cret"),
            ("OBC_MAPS_BASE_URL", BASE), ("OBC_CATALOG_URL", f"{BASE}/catalog.json"),
        ):
            previous = os.environ.get(name)
            os.environ[name] = value
            self.addCleanup(lambda n=name, p=previous: os.environ.__setitem__(n, p) if p
                            else os.environ.pop(n, None))
        real = r2.r2_client
        r2.r2_client = self.record
        self.addCleanup(lambda: setattr(r2, "r2_client", real))

    def body(self, key):
        """What the fake bucket hands back for one key."""

        if key == ARCHIVE_INDEX:
            return json.dumps(INDEX)
        if key == f"{PREFIX}/catalog.json":
            return json.dumps(CATALOG)
        if r2.BAND_INDEX.search(key):
            return json.dumps(CELL_INDEX if key == BAND_INDEX else {"cells": []})
        return "{}"

    def record(self, args, capture=False):
        """`obc data r2 ARGS` against the fake bucket."""

        self.calls.append(args)
        command, rest = args[0], [arg for arg in args[1:] if arg != "--json"]

        def rows(keys):
            return json.dumps({"bucket": "r2 bucket maps", "objects": [
                {"key": key, "bytes": self.bucket[key], "modified": "2026-09-01T10:11:12Z"} for key in keys]})

        if command == "list":
            return rows(key for key in sorted(self.bucket) if key.startswith(f"{rest[0]}/"))
        if command == "stat":
            return rows(key for key in rest if key in self.bucket)
        if command == "get":
            Path(rest[1]).write_text(self.body(rest[0]), encoding="utf-8")
        if command == "put":
            self.sent[rest[1]] = Path(rest[0]).read_text(encoding="utf-8")
        if command == "delete":
            for key in self.deleted_by(args):
                self.bucket.pop(key)
        return ""

    @staticmethod
    def deleted_by(args):
        return args[1:args.index("--reason")]

    def rm(self, *args):
        self.printed = io.StringIO()
        with redirect_stdout(self.printed):
            return r2.main(["rm", *args])

    def read_keys(self):
        return [args[1] for args in self.calls if args[0] == "get"]

    def deleted(self):
        return [key for args in self.calls if args[0] == "delete" for key in self.deleted_by(args)]

    def apply(self, key, *args, reason="a bad ingest"):
        return self.rm(key, *args, "--apply", "--reason", reason,
                       "--confirm", r2.confirmation([key]))

    # ── the plan ────────────────────────────────────────────────────────────

    def test_the_plan_states_what_the_live_listing_holds(self):
        self.assertEqual(self.rm(STRAY), 0)
        printed = self.printed.getvalue()
        self.assertIn(STRAY, printed)
        self.assertIn("r2 bucket maps: 1 object(s)", printed)  # the bucket the owner confirms
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
        self.assertNotIn("stat", [args[0] for args in self.calls])
        self.assertEqual(self.deleted(), [])

    def test_a_prefix_inside_the_cap_plans_every_object_under_it(self):
        self.assertEqual(self.rm("--prefix", "uploads"), 0)
        printed = self.printed.getvalue()
        self.assertIn(STRAY, printed)
        self.assertIn("uploads/older.obcm", printed)
        self.assertEqual(self.deleted(), [])

    # ── what the catalogue protects ─────────────────────────────────────────

    def test_the_catalogue_root_and_the_archive_index_refuse(self):
        for key, word in ((f"{PREFIX}/catalog.json", "catalogue root"),
                          (f"{PREFIX}/LICENSE.txt", "catalogue root"),
                          (f"{PREFIX}/regions/europe.json", "catalogue root"),
                          (ARCHIVE_INDEX, "reference archive index")):
            with self.subTest(key=key):
                self.assertEqual(self.rm(key), 1)
                self.assertIn(word, self.printed.getvalue())
                self.assertEqual(self.deleted(), [])

    def test_the_removal_log_is_never_deleted(self):
        self.bucket[r2.REMOVAL_LOG] = 40
        self.assertEqual(self.rm(r2.REMOVAL_LOG, "--i-mean-it", r2.REMOVAL_LOG), 1)
        self.assertEqual(self.calls, [])

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

    def test_the_index_goes_up_before_the_client_logs_and_deletes(self):
        self.assertEqual(self.apply(TILE), 0)
        order = [args[0] for args in self.calls if args[0] in ("put", "delete")]
        self.assertEqual(order, ["put", "delete"])
        self.assertEqual(list(self.sent), [ARCHIVE_INDEX])

        index = json.loads(self.sent[ARCHIVE_INDEX])
        self.assertEqual(index["tiles"], {"0100/0200": "es"})
        self.assertNotIn("3410/2882", index["sha256"])
        self.assertEqual(sorted(index["sources"]), ["es"])  # `ch` holds no other tile
        self.assertEqual(self.deleted(), [TILE])

    def test_the_reason_reaches_the_removal_log_of_the_client(self):
        """The client writes `removed.jsonl`; `rm` hands it the reason and its own consent."""

        self.assertEqual(self.apply(STRAY, reason="a stray upload"), 0)
        self.assertEqual(self.calls[-1], ["delete", STRAY, "--reason", "a stray upload", "--yes"])

    def test_the_archive_sits_where_the_ingest_tool_publishes_it(self):
        """One definition of the bucket, so a delete cannot land beside a publish."""

        self.assertEqual(r2.archive_prefix(), "reference/v1")
        self.assertEqual(r2.bucket_remote().path, "OBCR2:maps")
        self.assertEqual(r2.reference_tile(TILE), "3410/2882")
        self.assertIsNone(r2.reference_tile(CELL))


if __name__ == "__main__":
    unittest.main()
