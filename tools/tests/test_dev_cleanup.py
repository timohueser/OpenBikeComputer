import contextlib
import importlib.util
import io
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).parents[1] / "dev_cleanup.py"
SPEC = importlib.util.spec_from_file_location("dev_cleanup", MODULE_PATH)
cleanup = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = cleanup
SPEC.loader.exec_module(cleanup)


class DevCleanupTests(unittest.TestCase):
    def test_registered_worktrees_keeps_paths_with_newlines(self):
        output = "worktree /repo\0HEAD abc\0\0worktree /temp/obc-branch\nname\0locked active\0\0"
        with mock.patch.object(cleanup, "git") as git:
            git.return_value.stdout = output
            self.assertEqual(
                cleanup.registered_worktrees(Path("/repo")),
                {Path("/repo"), Path("/temp/obc-branch\nname")},
            )

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
            cleanup.git(root, "init", str(git_path))
            registered.mkdir()
            old = time.time() - 8 * cleanup.SECONDS_PER_DAY
            for path in (git_path / ".git", git_path, registered):
                os.utime(path, (old, old))
            self.assertEqual(
                cleanup.temp_candidates(time.time(), 7, root, excluded={registered}),
                [],
            )

    def test_temp_candidates_never_select_a_registered_worktree_parent(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            parent = root / "obc-review"
            parent.mkdir()
            worktree = parent / "OSM"
            cleanup.git(parent, "init", str(worktree))
            old = time.time() - 8 * cleanup.SECONDS_PER_DAY
            for path in (worktree, parent):
                os.utime(path, (old, old))

            self.assertEqual(
                cleanup.temp_candidates(time.time(), 7, root, excluded={worktree}),
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
            old = time.time() - 8 * cleanup.SECONDS_PER_DAY
            for path in [*bare.iterdir(), bare]:
                os.utime(path, (old, old))

            self.assertEqual(cleanup.temp_candidates(time.time(), 7, root), [])

    def test_main_dry_run_and_apply_only_remove_old_scratch(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            repo = root / "obc-worktree"
            cleanup.git(root, "init", str(repo))
            cache = repo / "target" / "debug" / "keep.rlib"
            cache.parent.mkdir(parents=True)
            cache.write_text("keep")
            old_scratch = root / "obc-pack-123-0-case"
            recent_scratch = root / "obcm-assemble-456-0-case"
            unrelated = root / "other-project"
            for path in (old_scratch, recent_scratch, unrelated):
                path.mkdir()
            old = time.time() - 8 * cleanup.SECONDS_PER_DAY
            for path in (repo / ".git", repo / "target", repo, old_scratch, unrelated):
                os.utime(path, (old, old))

            output = io.StringIO()
            with (
                mock.patch.object(cleanup.tempfile, "gettempdir", return_value=str(root)),
                contextlib.redirect_stdout(output),
            ):
                self.assertEqual(cleanup.main(["--repo", str(repo)]), 0)
                self.assertTrue(old_scratch.exists())
                self.assertIn(str(old_scratch), output.getvalue())
                self.assertEqual(cleanup.main(["--repo", str(repo), "--apply"]), 0)
            self.assertFalse(old_scratch.exists())
            self.assertTrue(recent_scratch.exists())
            self.assertTrue(unrelated.exists())
            self.assertEqual(cache.read_text(), "keep")

    def test_apply_skips_scratch_with_new_activity_after_planning(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            repo = root / "repo"
            cleanup.git(root, "init", str(repo))
            candidate = root / "obc-pack-123-0-case"
            candidate.mkdir()
            old = time.time() - 8 * cleanup.SECONDS_PER_DAY
            os.utime(candidate, (old, old))

            def new_activity(paths):
                (candidate / "active").touch()
                return {candidate: 0}

            with (
                mock.patch.object(cleanup.tempfile, "gettempdir", return_value=str(root)),
                mock.patch.object(cleanup, "directory_sizes", side_effect=new_activity),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(cleanup.main(["--repo", str(repo), "--apply"]), 0)
            self.assertTrue((candidate / "active").exists())


if __name__ == "__main__":
    unittest.main()
