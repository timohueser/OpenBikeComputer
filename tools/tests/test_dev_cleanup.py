import fcntl
import importlib.util
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).parents[1] / "dev_cleanup.py"
SPEC = importlib.util.spec_from_file_location("dev_cleanup", MODULE_PATH)
cleanup = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = cleanup
SPEC.loader.exec_module(cleanup)


class DevCleanupTests(unittest.TestCase):
    def test_parse_worktrees_keeps_safety_flags(self):
        parsed = cleanup.parse_worktrees(
            "worktree /repo\nHEAD abc\nbranch refs/heads/develop\n\n"
            "worktree /repo/wt\nHEAD def\ndetached\nlocked agent active\n\n"
        )
        self.assertEqual(len(parsed), 2)
        self.assertEqual(parsed[0].branch, "refs/heads/develop")
        self.assertTrue(parsed[1].detached)
        self.assertTrue(parsed[1].locked)

    def test_only_old_clean_merged_linked_worktree_is_eligible(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            main = root / "main"
            current = root / "current"
            linked = root / "linked"
            for path in (main, current, linked):
                path.mkdir()
            worktree = cleanup.Worktree(linked, head="abc", branch="refs/heads/topic")
            now = int(time.time())
            eligible = cleanup.classify_worktree(
                worktree,
                main_path=main,
                current_path=current,
                dirty=False,
                merged=True,
                activity_time=now - 8 * cleanup.SECONDS_PER_DAY,
                now=now,
                days=7,
            )
            self.assertTrue(eligible.eligible)

            for label, kwargs in {
                "dirty": {"dirty": True},
                "unmerged": {"merged": False},
                "recent": {"activity_time": now},
            }.items():
                values = {
                    "dirty": False,
                    "merged": True,
                    "activity_time": now - 8 * cleanup.SECONDS_PER_DAY,
                }
                values.update(kwargs)
                with self.subTest(label=label):
                    decision = cleanup.classify_worktree(
                        worktree,
                        main_path=main,
                        current_path=current,
                        now=now,
                        days=7,
                        **values,
                    )
                    self.assertFalse(decision.eligible)

    def test_main_current_and_locked_worktrees_are_never_eligible(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            main = root / "main"
            current = root / "current"
            other = root / "other"
            for path in (main, current, other):
                path.mkdir()
            now = int(time.time())
            for worktree in (
                cleanup.Worktree(main),
                cleanup.Worktree(current),
                cleanup.Worktree(other, locked=True),
            ):
                decision = cleanup.classify_worktree(
                    worktree,
                    main_path=main,
                    current_path=current,
                    dirty=False,
                    merged=True,
                    activity_time=0,
                    now=now,
                    days=0,
                )
                self.assertFalse(decision.eligible)

    def test_format_size_is_human_readable(self):
        self.assertEqual(cleanup.format_size(0), "0 B")
        self.assertEqual(cleanup.format_size(1536), "1.5 KiB")

    def test_temp_candidates_only_select_old_obc_namespaces(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            old_obc = root / "obc-pack-123-0-case"
            old_other = root / "other-project"
            recent_obc = root / "obcm-assemble-456-0-case"
            for path in (old_obc, old_other, recent_obc):
                path.mkdir()
            now = int(time.time())
            old = now - 8 * cleanup.SECONDS_PER_DAY
            for path in (old_obc, old_other):
                os.utime(path, (old, old))
            self.assertEqual(cleanup.temp_candidates(now, 7, root), [old_obc.resolve()])

    def test_temp_candidates_never_select_git_or_registered_worktrees(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            git_path = root / "obc-review-clone"
            registered = root / "obc-registered-worktree"
            for path in (git_path, registered):
                path.mkdir()
                old = time.time() - 8 * cleanup.SECONDS_PER_DAY
                os.utime(path, (old, old))
            (git_path / ".git").touch()
            self.assertEqual(
                cleanup.temp_candidates(int(time.time()), 7, root, excluded={registered}),
                [],
            )

    def test_temp_candidates_never_follow_symlinks_outside_temp(self):
        with tempfile.TemporaryDirectory() as scratch, tempfile.TemporaryDirectory() as outside:
            root = Path(scratch)
            target = Path(outside) / "important"
            target.mkdir()
            sentinel = target / "keep.txt"
            sentinel.write_text("keep")
            (root / "obc-old").symlink_to(target, target_is_directory=True)

            self.assertEqual(cleanup.temp_candidates(int(time.time()), 0, root), [])
            self.assertEqual(sentinel.read_text(), "keep")

    def test_temp_candidates_never_select_bare_git_repositories(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            bare = root / "obc-review.git"
            cleanup.git(root, "init", "--bare", str(bare))

            self.assertEqual(cleanup.temp_candidates(int(time.time()), 0, root), [])

    def test_stale_artifacts_selects_old_entries_in_every_profile_dir(self):
        with tempfile.TemporaryDirectory() as scratch:
            target = Path(scratch)
            now = int(time.time())
            old = now - 8 * cleanup.SECONDS_PER_DAY
            profiles = [target / "debug", target / "thumbv8m.main-none-eabihf" / "debug"]
            for profile in profiles:
                for name in cleanup.ARTIFACT_DIRS:
                    (profile / name).mkdir(parents=True)
                (profile / ".cargo-lock").touch()
            host, device = profiles
            old_rlib = host / "deps" / "libserde-old.rlib"
            new_rlib = host / "deps" / "libserde-new.rlib"
            old_session = host / "incremental" / "obc_app-old"
            live_session = host / "incremental" / "obc_app-live"
            uplifted = host / "obc-sim"
            device_fingerprint = device / ".fingerprint" / "obc-fw-old"
            for path in (old_rlib, new_rlib, uplifted):
                path.touch()
            for path in (old_session, live_session, device_fingerprint):
                path.mkdir()
            (live_session / "s-new").mkdir()
            for path in (old_rlib, old_session, live_session, uplifted, device_fingerprint, host / ".cargo-lock"):
                os.utime(path, (old, old))

            stale = cleanup.stale_artifacts(target, now - 7 * cleanup.SECONDS_PER_DAY)

            self.assertEqual(
                stale,
                {host: [old_rlib, old_session, uplifted], device: [device_fingerprint]},
            )

    def test_remove_stale_artifacts_yields_to_a_running_build(self):
        with tempfile.TemporaryDirectory() as scratch:
            profile = Path(scratch)
            lock_path = profile / ".cargo-lock"
            lock_path.touch()
            stale = profile / "deps" / "libold.rlib"
            stale.parent.mkdir()
            stale.touch()
            cutoff = time.time() + 1

            with open(lock_path) as build:
                fcntl.flock(build, fcntl.LOCK_EX)
                self.assertFalse(cleanup.remove_stale_artifacts(profile, [stale], cutoff))
                self.assertTrue(stale.exists())

            self.assertTrue(cleanup.remove_stale_artifacts(profile, [stale], cutoff))
            self.assertFalse(stale.exists())


if __name__ == "__main__":
    unittest.main()
