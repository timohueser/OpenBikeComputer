import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).parents[1] / "check_paths.py"
SPEC = importlib.util.spec_from_file_location("check_paths", MODULE_PATH)
check_paths = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = check_paths
SPEC.loader.exec_module(check_paths)

RECIPES = "[group('test')]\ntest *args:\n    echo\n\ncheck-docs:\n    echo\n"


class CheckPathsTests(unittest.TestCase):
    def problems(self, files: dict[str, str]) -> list[str]:
        """The problems of a tree that holds `files`, a justfile and an empty guide."""
        tree = {
            "AGENTS.md": "",
            "tools/README.md": "",
            "tools/justfile": RECIPES,
            "docs/README.md": "",
            "docs/content/nav.json": '{"sections": [{"pages": [{"path": "index"}]}]}',
            "docs/content/index.md": "",
            ".gitignore": "target/\n",
            **files,
        }
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name, text in tree.items():
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_text(text)
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(["git", "add", "-A"], cwd=root, check=True)
            with mock.patch.object(check_paths, "ROOT", root):
                return check_paths.check()

    def test_a_page_names_only_paths_that_exist(self):
        page = (
            "`tools/justfile` `justfile` from this directory, `tools/*file`, the output `target/bom`, "
            "the layer `planner/routing`, the URL path `/plan/`, `src/lib.rs` of some crate.\n"
            "`tools/gone.py` and `docs/content/gone/`.\n"
            "```sh\ncat tools/not-checked-in-a-fence.py\n```\n"
        )
        self.assertEqual(
            self.problems({"tools/README.md": page, "planner/README.md": ""}),
            ["tools/README.md: `tools/gone.py` does not exist", "tools/README.md: `docs/content/gone/` does not exist"],
        )

    def test_a_page_names_only_tasks_that_exist(self):
        page = "Run `obc test -p CRATE`, `obc help test` or `obc TASK --help`.\n```sh\n./tools/obc gone --now\nobc check-docs\n```\n"
        self.assertEqual(
            self.problems({"AGENTS.md": page}),
            ["AGENTS.md: `obc gone` is not a task of tools/justfile"],
        )

    def test_each_top_level_directory_has_a_page_inside_its_cap(self):
        self.assertEqual(
            self.problems({"AGENTS.md": "x\n" * 151, "docs/README.md": "x\n" * 251, "sim/main.rs": ""}),
            [
                "AGENTS.md: 151 lines, the cap is 150",
                "docs/README.md: 251 lines, the cap is 250",
                "sim/README.md: each top-level directory needs this page",
            ],
        )

    def test_links_resolve_and_guide_pages_are_published(self):
        self.assertEqual(
            self.problems({
                "sim/README.md": "[ok](../tools/justfile) [far](https://example.com) [gone](gone.md#part)\n",
                "docs/content/lost.md": "[code](src:tools/justfile) [gone](src:tools/gone.py) [old](spec:x.md)\n",
                "docs/content/blog/post/index.md": "",
            }),
            [
                "docs/content/lost.md: under docs/content/ but nav.json does not publish it",
                "docs/content/lost.md: src:tools/gone.py does not exist",
                "docs/content/lost.md: `spec:` is not a link scheme; use `src:specs/x.md`",
                "sim/README.md: link to gone.md does not resolve",
            ],
        )


if __name__ == "__main__":
    unittest.main()
