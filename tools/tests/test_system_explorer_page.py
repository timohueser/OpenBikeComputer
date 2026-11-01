"""The system explorer page takes its data and its system pages from the map."""

import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from docs import build_docs


class SystemExplorerPageTests(unittest.TestCase):
    def test_a_system_doc_links_the_site_and_the_repository(self):
        md = "[a](routing.md#why) [b](../content/hardware/index.md) [c](../../firmware/x.rs) [d](#here) [e](https://x.org/a.md)"
        self.assertEqual(build_docs.system_doc_links(md),
                         "[a](../routing/#why) [b](../../hardware/) [c](src:firmware/x.rs) [d](#here) [e](https://x.org/a.md)")

    def test_the_data_block_cannot_close_its_script(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "map.json"
            path.write_text(json.dumps({"systems": [{"purpose": "</script><b>"}]}))
            with mock.patch.object(build_docs, "SYSTEM_MAP", path):
                page = build_docs.system_explorer()
        block = page.split('id="system-map">')[1].split("</script>")[0]
        self.assertEqual(json.loads(block)["systems"][0]["purpose"], "</script><b>")

    def test_the_sidebar_lists_only_the_open_system(self):
        nav = {"sections": [{"title": "Systems", "pages": [{"path": build_docs.EXPLORER_PAGE, "title": "Explorer"}]},
                            {"title": "Other", "pages": []}]}
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "map.json"
            path.write_text(json.dumps({"systems": [{"name": "a"}, {"name": "b"}]}))
            with mock.patch.object(build_docs, "SYSTEM_MAP", path):
                pages = build_docs.system_pages(nav)
        self.assertEqual([p[0] for p in pages], ["systems/a", "systems/b"])
        self.assertEqual([pg["path"] for pg in pages[1][2]["sections"][0]["pages"]], [build_docs.EXPLORER_PAGE, "systems/b"])
        self.assertEqual(pages[1][2]["sections"][1]["pages"], [])


if __name__ == "__main__":
    unittest.main()
