import contextlib
import fcntl
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
    def setUp(self):
        # Never let a test reach the developer's real shared cargo target.
        self.enterContext(mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": ""}))

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

            target = self.make_target(
                root / "cargo-target",
                {
                    "CACHEDIR.TAG": (old, old),
                    "debug/.cargo-lock": (old, old),
                    "debug/.fingerprint/old-1111111111111111/lib-old": (old, old),
                    "debug/deps/libold-1111111111111111.rlib": (old, old),
                },
            )
            old_rlib = target / "debug" / "deps" / "libold-1111111111111111.rlib"

            output = io.StringIO()
            with (
                mock.patch.object(cleanup.tempfile, "gettempdir", return_value=str(root)),
                mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": str(target)}),
                contextlib.redirect_stdout(output),
            ):
                self.assertEqual(cleanup.main(["--repo", str(repo)]), 0)
                self.assertTrue(old_scratch.exists())
                self.assertTrue(old_rlib.exists())
                self.assertIn(str(old_scratch), output.getvalue())
                self.assertIn(str(old_rlib.parent), output.getvalue())
                self.assertEqual(cleanup.main(["--repo", str(repo), "--apply"]), 0)
            self.assertFalse(old_scratch.exists())
            self.assertFalse(old_rlib.exists())
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

    def make_target(self, root: Path, files: dict[str, tuple[float, float]]) -> Path:
        """A fake cargo target with old directories and files at the given (atime, mtime)."""
        target = root.resolve()
        for name in files:
            (target / name).parent.mkdir(parents=True, exist_ok=True)
            (target / name).write_text("x")
        old = time.time() - 8 * cleanup.SECONDS_PER_DAY
        for directory, _, _ in os.walk(target):
            os.utime(directory, (old, old))
        for name, times in files.items():
            os.utime(target / name, times)
        return target

    def test_prune_cargo_target_removes_unused_units_whole_inside_the_target(self):
        with tempfile.TemporaryDirectory() as scratch, tempfile.TemporaryDirectory() as outside:
            now = time.time()
            old = now - 8 * cleanup.SECONDS_PER_DAY
            wasm = "wasm32-unknown-unknown/debug"
            target = self.make_target(
                Path(scratch),
                {
                    "debug/.cargo-lock": (old, old),
                    # Every build reads the fingerprint but only stats the other outputs.
                    "debug/.fingerprint/live-1111111111111111/lib-live": (now, old),
                    "debug/build/live-1111111111111111/out/gen.rs": (old, old),
                    "debug/deps/liblive-1111111111111111.rmeta": (old, old),
                    "debug/.fingerprint/stale-2222222222222222/lib-stale": (old, old),
                    "debug/build/stale-2222222222222222/output": (old, old),
                    "debug/deps/libstale-2222222222222222.rlib": (old, old),
                    "debug/deps/stale-2222222222222222.d": (old, old),
                    "debug/obc-data": (old, old),
                    # Cargo leaves the hash out of a path package's cdylib and overwrites it.
                    "debug/deps/libcdy_lib.so": (old, old),
                    "debug/deps/cdy_lib.d": (old, old),
                    f"{wasm}/deps/cdy_lib.wasm": (old, old),
                    "debug/incremental/stale-3333333333333333/s-1/dep-graph.bin": (old, old),
                    "debug/incremental/live-4444444444444444/s-2/dep-graph.bin": (now, now),
                    "debug/incremental/live-4444444444444444/s-2/object.o": (old, old),
                    f"{wasm}/.cargo-lock": (old, old),
                    f"{wasm}/.fingerprint/w-5555555555555555/lib-w": (old, old),
                    # A profile without `.cargo-lock` is not cargo's.
                    "doc/.fingerprint/d-6666666666666666/lib-d": (old, old),
                    "doc/deps/libd-6666666666666666.rlib": (old, old),
                },
            )
            outside_file = Path(outside) / "w-5555555555555555" / "output"
            outside_file.parent.mkdir()
            outside_file.write_text("x")
            os.utime(outside_file, (old, old))
            (target / wasm / "build").symlink_to(outside, target_is_directory=True)
            cutoff = now - 7 * cleanup.SECONDS_PER_DAY
            expected = (
                {
                    target / "debug" / ".fingerprint": (1, 1),
                    target / "debug" / "build": (1, 1),
                    target / "debug" / "deps": (2, 2),
                    target / "debug" / "incremental": (1, 1),
                    target / wasm / ".fingerprint": (1, 1),
                },
                [],
            )

            self.assertEqual(cleanup.prune_cargo_target(target, cutoff, apply=False), expected)
            self.assertTrue((target / "debug" / "deps" / "libstale-2222222222222222.rlib").exists())

            self.assertEqual(cleanup.prune_cargo_target(target, cutoff, apply=True), expected)
            for gone in (
                "debug/.fingerprint/stale-2222222222222222",
                "debug/build/stale-2222222222222222",
                "debug/deps/libstale-2222222222222222.rlib",
                "debug/deps/stale-2222222222222222.d",
                "debug/incremental/stale-3333333333333333",
                f"{wasm}/.fingerprint/w-5555555555555555",
            ):
                self.assertFalse((target / gone).exists(), gone)
            for kept in (
                "debug/.fingerprint/live-1111111111111111/lib-live",
                "debug/build/live-1111111111111111/out/gen.rs",
                "debug/deps/liblive-1111111111111111.rmeta",
                "debug/obc-data",
                "debug/deps/libcdy_lib.so",
                "debug/deps/cdy_lib.d",
                f"{wasm}/deps/cdy_lib.wasm",
                "debug/incremental/live-4444444444444444/s-2/object.o",
                "doc/deps/libd-6666666666666666.rlib",
            ):
                self.assertTrue((target / kept).exists(), kept)
            self.assertTrue(outside_file.exists())

    def test_prune_cargo_target_skips_a_profile_that_a_build_holds(self):
        with tempfile.TemporaryDirectory() as scratch:
            old = time.time() - 8 * cleanup.SECONDS_PER_DAY
            target = self.make_target(
                Path(scratch),
                {
                    "debug/.cargo-lock": (old, old),
                    "debug/.fingerprint/old-1111111111111111/lib-old": (old, old),
                    "debug/deps/libold-1111111111111111.rlib": (old, old),
                },
            )
            cutoff = time.time() - 7 * cleanup.SECONDS_PER_DAY
            with open(target / "debug" / ".cargo-lock") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                self.assertEqual(
                    cleanup.prune_cargo_target(target, cutoff, apply=True), ({}, [target / "debug"])
                )
            self.assertTrue((target / "debug" / "deps" / "libold-1111111111111111.rlib").exists())
            self.assertEqual(
                cleanup.prune_cargo_target(target, cutoff, apply=True)[0],
                {target / "debug" / ".fingerprint": (1, 1), target / "debug" / "deps": (1, 1)},
            )

    def test_cargo_target_refuses_root_home_and_repositories_and_skips_untagged_dirs(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            repo = root / "repo"
            cleanup.git(root, "init", str(repo))
            for value in ("/", str(Path.home()), str(repo)):
                with self.assertRaises(cleanup.CleanupError):
                    cleanup.cargo_target(value)
            untagged = root / "untagged"
            untagged.mkdir()
            with contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertIsNone(cleanup.cargo_target(str(untagged)))
            self.assertIn("no CACHEDIR.TAG", output.getvalue())
            (untagged / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55")
            self.assertEqual(cleanup.cargo_target(str(untagged)), untagged.resolve())
            self.assertIsNone(cleanup.cargo_target(""))
            self.assertIsNone(cleanup.cargo_target(str(root / "missing")))


if __name__ == "__main__":
    unittest.main()
