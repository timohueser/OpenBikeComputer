"""The installed entry point selects local tools without changing argument paths."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class ObcTests(unittest.TestCase):
    def test_documentation_recipe_uses_checkout_root_and_propagates_failure(self):
        source = Path(__file__).parents[2]
        with tempfile.TemporaryDirectory(prefix="obc docs ") as directory:
            root = Path(directory).resolve()
            (root / "docs").mkdir()
            (root / "tools").mkdir()
            caller = root / "nested caller"
            caller.mkdir()
            shutil.copyfile(source / "justfile", root / "justfile")
            shutil.copyfile(source / "tools/justfile", root / "tools/justfile")
            (root / "docs/build_docs.py").write_text(
                "import json,os,sys\nfrom pathlib import Path\n"
                "print(json.dumps([str(Path.cwd()),sys.argv[1:]]))\n"
                "raise SystemExit(int(os.environ['DOCS_EXIT']))\n"
            )
            for entry in (root / "justfile", root / "tools/justfile"):
                for code in (0, 7):
                    with self.subTest(entry=entry, code=code):
                        result = subprocess.run(
                            ["just", "--justfile", str(entry), "check-docs"], cwd=caller,
                            env={**os.environ, "DOCS_EXIT": str(code)}, capture_output=True, text=True,
                        )
                        self.assertEqual(result.returncode, code, result.stderr)
                        self.assertEqual(json.loads(result.stdout), [str(root), ["--check-links"]])

    def test_installed_command_selects_worktree_and_preserves_global_fallback(self):
        with tempfile.TemporaryDirectory(prefix="obc entry ") as temporary:
            root = Path(temporary).resolve()
            main = root / "main"
            (main / "tools").mkdir(parents=True)
            (main / "firmware/obc-app").mkdir(parents=True)
            (main / "firmware/obc-app/.keep").touch()
            shutil.copyfile(Path(__file__).parents[1] / "obc", main / "tools/obc")
            (main / "tools/obc").chmod(0o755)
            (main / "tools/justfile").touch()
            def git(*args):
                subprocess.run(["git", "-C", str(main), *args], check=True, capture_output=True)
            git("init")
            git("add", ".")
            git("-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-m", "fixture")
            worktree = root / "linked checkout"
            git("worktree", "add", "-b", "topic", str(worktree))
            (worktree / "justfile").write_text("# root recipes\n")
            (worktree / "tools/justfile").unlink()
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
                    justfile = selected / ("justfile" if selected == worktree else "tools/justfile")
                    self.assertEqual(json.loads(result.stdout), [
                        "--justfile", str(justfile), "--working-directory", str(cwd),
                        "sim", "a map.obcm", "--", "--heading", "90",
                    ])
                    self.assertIn(f"obc: checkout {selected}", result.stderr)
                    completion = subprocess.run(
                        ["bash", "-c", 'source "$1"; _obc_toolsdir', "completion",
                         str(Path(__file__).parents[1] / "obc.bash")],
                        cwd=cwd, env=env, capture_output=True, text=True, check=True,
                    )
                    self.assertEqual(completion.stdout.strip(), str(selected / "tools"))

    def test_native_and_installed_entry_points_preserve_root_and_caller_paths(self):
        source = Path(__file__).parents[2]
        with tempfile.TemporaryDirectory(prefix="obc native ") as directory:
            root = Path(directory).resolve()
            (root / "tools").mkdir()
            (root / "firmware/obc-app").mkdir(parents=True)
            caller = root / "nested caller"
            caller.mkdir()
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            shutil.copyfile(source / "justfile", root / "justfile")
            for name in ("justfile", "obc", "obc-dev.sh"):
                shutil.copyfile(source / "tools" / name, root / "tools" / name)
            (root / "tools/verification").mkdir()
            (root / "tools/verification/req.py").write_text(
                "import json,os,sys\nfrom pathlib import Path\n"
                "print(json.dumps([os.environ['OBC_ROOT'],os.environ['OBC_TOOLS'],str(Path.cwd()),sys.argv[1:]]))\n"
            )
            args = ["../a map.obcm", "--check"]
            commands = [
                ["just", "req", *args],
                ["just", "--justfile", str(root / "tools/justfile"), "req", *args],
                ["bash", str(root / "tools/obc"), "req", *args],
            ]
            env = dict(os.environ)
            env.pop("OBC_DRY_RUN", None)
            for command in commands:
                with self.subTest(entry=command[:2]):
                    result = subprocess.run(command, cwd=caller, env=env,
                                            capture_output=True, text=True, check=True)
                    self.assertEqual(json.loads(result.stdout),
                                     [str(root), str(root / "tools"), str(caller), args])

    def test_secrets_come_from_the_credentials_file_and_obc_local_names_its_own(self):
        source = Path(__file__).parents[1]
        with tempfile.TemporaryDirectory(prefix="obc secrets ") as directory:
            root, home = Path(directory) / "repo", Path(directory) / "home"
            (root / "tools").mkdir(parents=True)
            (home / ".config/openbikecomputer").mkdir(parents=True)
            shutil.copyfile(source / "obc-dev.sh", root / "tools/obc-dev.sh")
            (home / ".config/openbikecomputer/credentials.env").write_text("OBC_KEPT_TOKEN=from-credentials\n")
            (root / "tools/obc.local").write_text("OBC_LEFT_TOKEN=do-not-print\nOBC_MAP=a.obcm\n")
            script = 'source tools/obc-dev.sh; obc_init; echo "$OBC_KEPT_TOKEN $OBC_MAP"'
            env = {**os.environ, "HOME": str(home), "OBC_ROOT": str(root), "OBC_TOOLS": str(root / "tools")}
            result = subprocess.run(["bash", "-c", script], cwd=root, env=env, capture_output=True, text=True, check=True)
            self.assertEqual(result.stdout.strip(), "from-credentials a.obcm")
            self.assertIn("OBC_LEFT_TOKEN", result.stderr)
            self.assertNotIn("do-not-print", result.stderr + result.stdout)
