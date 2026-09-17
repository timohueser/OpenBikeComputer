"""The installed entry point selects local tools without changing argument paths."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class ObcTests(unittest.TestCase):
    def test_installed_command_selects_worktree_and_preserves_global_fallback(self):
        with tempfile.TemporaryDirectory(prefix="obc entry ") as temporary:
            root = Path(temporary)
            main = root / "main"
            (main / "tools").mkdir(parents=True)
            (main / "firmware/obc-app").mkdir(parents=True)
            (main / "firmware/obc-app/.keep").touch()
            shutil.copyfile(Path(__file__).parents[1] / "obc", main / "tools/obc")
            (main / "tools/justfile").touch()
            def git(*args):
                subprocess.run(["git", "-C", str(main), *args], check=True, capture_output=True)
            git("init")
            git("add", ".")
            git("-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-m", "fixture")
            worktree = root / "linked checkout"
            git("worktree", "add", "-b", "topic", str(worktree))
            unrelated = root / "unrelated"
            unrelated.mkdir()
            subprocess.run(["git", "init", str(unrelated)], check=True, capture_output=True)
            binary = root / "bin"
            binary.mkdir()
            (binary / "obc").symlink_to(main / "tools/obc")
            just = binary / "just"
            just.write_text("#!/usr/bin/env python3\nimport json, sys\nprint(json.dumps(sys.argv[1:]))\n")
            just.chmod(0o755)
            env = {**os.environ, "PATH": str(binary) + os.pathsep + os.environ["PATH"]}
            for cwd, selected in [(main, main), (worktree / "firmware", worktree), (root, main), (unrelated, main)]:
                with self.subTest(cwd=cwd):
                    result = subprocess.run(
                        ["bash", str(binary / "obc"), "sim", "a map.obcm", "--", "--heading", "90"],
                        cwd=cwd, env=env, capture_output=True, text=True, check=True,
                    )
                    self.assertEqual(json.loads(result.stdout), [
                        "--justfile", str(selected / "tools/justfile"), "--working-directory", str(cwd),
                        "sim", "a map.obcm", "--", "--heading", "90",
                    ])
                    self.assertIn(f"obc: checkout {selected}", result.stderr)
