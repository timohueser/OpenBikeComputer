"""Download links are checked against the files in the generated site."""

from contextlib import redirect_stdout, redirect_stderr
from io import StringIO
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from docs import build_docs


class HardwareDocsTests(unittest.TestCase):
    def test_downloads_must_exist_and_page_fragments_still_need_headings(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(build_docs, "OUT", Path(tmp)):
            output = Path(tmp) / "hardware" / "bom"
            output.mkdir(parents=True)
            (output / "main.pdf").write_bytes(b"%PDF-1.5")
            content = '<h2 id="schematics">Schematics</h2><a href="main.pdf">PDF</a>'
            url = "docs/hardware/bom/"
            with redirect_stdout(StringIO()), redirect_stderr(StringIO()):
                self.assertEqual(build_docs.check_links({url: content}), 0)
                self.assertEqual(build_docs.check_links({url: content + '<a href="missing.csv">CSV</a>'}), 1)
                self.assertEqual(build_docs.check_links({url: content + '<a href="#schematics">Sheets</a>'}), 0)
                self.assertEqual(build_docs.check_links({url: content + '<a href="#missing">Missing</a>'}), 1)


if __name__ == "__main__":
    unittest.main()
