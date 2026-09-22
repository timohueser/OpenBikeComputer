"""`obc r2 rm`, with rclone replaced by a recorder.

rclone is not a test dependency, so what the tests hold is what this tool owns: what the
plan says, what lets it run, what it refuses to touch, and the order the calls go out in.
No test reaches the network.
"""

import importlib.util
import io
import json
import os
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "r2.py"
SPEC = importlib.util.spec_from_file_location("r2", MODULE_PATH)
r2 = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = r2
SPEC.loader.exec_module(r2)


TILE = "obc/reference/v1/16/3410/2882.tif"
CELL = "obc/cells/fine/1204/1052.obcm"
STRAY = "uploads/scratch.obcm"

#: What the fake bucket holds, by key.
BUCKET = {
    TILE: 2097152,
    CELL: 4823910,
    STRAY: 17,
    "uploads/older.obcm": 21,
    "obc/catalog.json": 8100,
    "obc/regions/europe.json": 640,
    "obc/reference/v1/index.json": 900,
}

INDEX = {
    "schema": 1, "step_log2": 6, "tile_log2": 16,
    "sources": {"ch": {"product": "swissALTI3D"}, "es": {"product": "MDT05"}},
    "tiles": {"3410/2882": "ch", "0100/0200": "es"},
    "contributors": {"3410/2882": ["ch"], "0100/0200": ["es"]},
    "sha256": {"3410/2882": "aa" * 32, "0100/0200": "bb" * 32},
}

CATALOG = {"cells": [{"key": "cells/fine/1204/1052.obcm", "bytes": 4823910}]}


class Rm(unittest.TestCase):
    def setUp(self):
        self.calls, self.sent = [], {}
        self.bucket = dict(BUCKET)
        self.log = None
        for name, value in (
            ("OBC_R2_ACCOUNT_ID", "acc"), ("OBC_R2_BUCKET", "maps"), ("OBC_R2_PREFIX", "/obc/"),
            ("OBC_R2_ACCESS_KEY_ID", "key"), ("OBC_R2_SECRET_ACCESS_KEY", "s3cret"),
        ):
            previous = os.environ.get(name)
            os.environ[name] = value
            self.addCleanup(lambda n=name, p=previous: os.environ.__setitem__(n, p) if p
                            else os.environ.pop(n, None))
        real = r2.run_rclone
        r2.run_rclone = self.record
        self.addCleanup(lambda: setattr(r2, "run_rclone", real))

    #: Everything the fake bucket can hand back, by key.
    def body(self, key):
        if key == "obc/reference/v1/index.json":
            return json.dumps(INDEX)
        if key == "obc/catalog.json":
            return json.dumps(CATALOG)
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

    def verbs(self):
        return [argv[0] for argv, _ in self.calls]

    def deleted(self):
        return [argv[1].removeprefix("OBCR2:maps/") for argv, _ in self.calls if argv[0] == "deletefile"]

    def apply(self, key, *args, reason="a bad ingest"):
        return self.rm(key, *args, "--apply", "--reason", reason, "--confirm", f"1 {key}")

    def test_the_plan_states_what_the_live_listing_holds(self):
        self.assertEqual(self.rm(STRAY), 0)
        printed = self.printed.getvalue()
        self.assertIn(STRAY, printed)
        self.assertIn("17", printed)                     # the size, from the listing
        self.assertIn("2026-09-01T10:11:12Z", printed)   # and the last-modified
        self.assertIn(f'--confirm "1 {STRAY}"', printed)
        self.assertEqual(self.deleted(), [])

    def test_a_key_the_bucket_does_not_hold_is_an_error(self):
        self.assertEqual(self.rm(STRAY, "uploads/gone.obcm"), 1)
        self.assertEqual(self.deleted(), [])

    def test_the_bucket_root_is_never_a_target(self):
        for prefix in ("", "/", ".", "../obc"):
            with self.subTest(prefix=prefix):
                self.assertEqual(self.rm("--prefix", prefix), 1)
        self.assertEqual(self.rm(STRAY, "--prefix", "obc"), 1)  # one form, never both
        self.assertEqual(self.deleted(), [])

    def test_a_prefix_wider_than_one_call_takes_is_refused(self):
        self.bucket = {f"uploads/{n:04d}.obcm": 10 for n in range(r2.RM_CAP + 1)}
        self.assertEqual(self.rm("--prefix", "uploads"), 1)
        self.assertNotIn("lsjson --files-from", " ".join(self.verbs()))
        self.assertEqual(self.deleted(), [])

    def test_a_prefix_inside_the_cap_plans_every_object_under_it(self):
        self.assertEqual(self.rm("--prefix", "uploads"), 0)
        printed = self.printed.getvalue()
        self.assertIn(STRAY, printed)
        self.assertIn("uploads/older.obcm", printed)
        self.assertEqual(self.deleted(), [])

    def test_the_catalogue_root_and_the_archive_index_refuse(self):
        for key, word in (("obc/catalog.json", "catalogue root"),
                          ("obc/regions/europe.json", "catalogue root"),
                          ("obc/reference/v1/index.json", "reference archive index")):
            with self.subTest(key=key):
                self.assertEqual(self.rm(key), 1)
                self.assertIn(word, self.printed.getvalue())
                self.assertEqual(self.deleted(), [])

    def test_an_object_the_live_catalogue_names_refuses_until_it_is_meant(self):
        self.assertEqual(self.rm(CELL), 1)
        self.assertIn("live catalogue names it", self.printed.getvalue())

        self.assertEqual(self.apply(CELL, "--i-mean-it", CELL), 0)
        self.assertEqual(self.deleted(), [CELL])
        self.assertIn(r2.REPUBLISH, self.printed.getvalue())

    def test_i_mean_it_has_to_name_a_key_this_plan_protects(self):
        self.assertEqual(self.rm(STRAY, "--i-mean-it", "obc/catalog.json"), 1)
        self.assertEqual(self.deleted(), [])

    def test_apply_needs_the_plans_own_confirmation_and_a_reason(self):
        for extra in ([], ["--confirm", f"1 {STRAY}"], ["--confirm", "2 " + STRAY, "--reason", "x"],
                      ["--reason", "x", "--confirm", f"1 {CELL}"]):
            with self.subTest(extra=extra):
                self.assertEqual(self.rm(STRAY, "--apply", *extra), 1)
                self.assertEqual(self.deleted(), [])

    def test_a_reference_tile_is_unnamed_before_its_object_goes(self):
        self.assertEqual(self.apply(TILE), 0)
        writes = [n for n, (argv, _) in enumerate(self.calls)
                  if argv[0] == "copyto" and argv[2].startswith("OBCR2:")]
        deletes = [n for n, (argv, _) in enumerate(self.calls) if argv[0] == "deletefile"]
        self.assertLess(max(writes), min(deletes))  # the index and the log go first
        self.assertEqual([self.calls[n][0][2].removeprefix("OBCR2:maps/") for n in writes],
                         ["obc/reference/v1/index.json", r2.REMOVAL_LOG])

        index = json.loads(self.sent["obc/reference/v1/index.json"])
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

        self.assertEqual(r2.archive_prefix(), "obc/reference/v1")
        self.assertEqual(r2.bucket_remote().path, "OBCR2:maps")
        self.assertEqual(r2.reference_tile(TILE), "3410/2882")
        self.assertIsNone(r2.reference_tile(CELL))


if __name__ == "__main__":
    unittest.main()
