"""Purchasing quantities, incomplete quotes and reel constraints."""

import csv
from decimal import Decimal
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from hardware import bom


def part(board, qty, mpn="R1", prices="1:0.12;50:0.08;1000@1000:0.04"):
    return {"Board": board, "Reference": "R1", "Qty": str(qty), "Value": "10k",
            "Manufacturer": "Maker", "MPN": mpn, "Price breaks EUR": prices,
            "Price source": "https://www.mouser.de/example", "Price checked": "2026-10-02"}


class HardwareBomTests(unittest.TestCase):
    def test_native_exports_include_every_board_and_all_pdf_sheets(self):
        def cli(command, check):
            output = Path(command[command.index("--output") + 1])
            if command[3] == "bom":
                row = part("", 1)
                row.pop("Board")
                with output.open("w", newline="") as stream:
                    writer = csv.DictWriter(stream, fieldnames=row)
                    writer.writeheader()
                    writer.writerow(row)
            else:
                self.assertNotIn("--pages", command)
                output.write_bytes(b"%PDF-1.5")

        with tempfile.TemporaryDirectory() as tmp, patch.object(bom.subprocess, "run", side_effect=cli) as run:
            output = Path(tmp)
            content = bom.export(output, "kicad-cli", schematics=True)
            self.assertEqual(run.call_count, 4)
            for board in bom.BOARDS:
                for suffix in ("csv", "pdf"):
                    name = f"{board}.{suffix}"
                    self.assertTrue((output / name).is_file())
                    self.assertIn(f'href="{name}"', content)
            self.assertNotIn("<html", content)
            self.assertIn('href="purchasing.csv"', content)
            with (output / "purchasing.csv").open(encoding="utf-8-sig") as stream:
                rows = list(csv.DictReader(stream))
            self.assertEqual({row["Board"] for row in rows}, set(bom.BOARDS))

    def test_shared_parts_reach_break_with_combined_board_quantity(self):
        main, usb = bom.cost_rows([part("main", 2), part("usb", 3)])
        self.assertEqual(main["Unit EUR / 10 devices"], "0.08")
        self.assertEqual(usb["Unit EUR / 10 devices"], "0.08")
        self.assertEqual(main["Line EUR / 10 devices"], "1.60")
        self.assertEqual(usb["Line EUR / 10 devices"], "2.40")
        self.assertEqual(main["Line EUR / 1000 devices"], "80.00")

    def test_reel_discount_requires_whole_reels(self):
        breaks = "1:0.12;50:0.08;1000@1000:0.04"
        self.assertEqual(bom.unit_price(breaks, 1500), Decimal("0.08"))
        self.assertEqual(bom.unit_price(breaks, 2000), Decimal("0.04"))
        self.assertIsNone(bom.unit_price("1000@1000:0.04", 10))
        for invalid in ("0:1", "1@0:1", "1:NaN", "1:-1"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                bom.unit_price(invalid, 1000)

    def test_conflicting_or_unattributed_prices_stop_export(self):
        with self.assertRaisesRegex(ValueError, "Conflicting prices"):
            bom.cost_rows([part("main", 1), part("usb", 1, prices="1:0.2")])
        unattributed = part("main", 1)
        unattributed["Price source"] = ""
        with self.assertRaisesRegex(ValueError, "lacks source"):
            bom.cost_rows([unattributed])

    def test_unknown_quote_is_blank_and_report_never_calls_subtotal_total(self):
        rows = bom.cost_rows([part("main", 1), part("usb", 1, mpn="<unquoted>", prices="")])
        self.assertEqual(rows[1]["Line EUR / 10 devices"], "")
        rows[1]["Capacitance at bias"] = ">=5uF"
        reviews = {key: f"Internal {key}" for key in
                   ("Selection notes", "Sourcing notes", "Tuning", "Footprint status", "Design status")}
        rows[1].update(reviews)
        with tempfile.TemporaryDirectory() as tmp:
            bom.write_report(rows, Path(tmp))
            html = (Path(tmp) / "index.html").read_text()
            self.assertIn("Known-price subtotal", html)
            self.assertNotIn("Estimated parts total", html)
            self.assertIn("1 unpriced rows", html)
            self.assertIn("&lt;unquoted&gt;", html)
            self.assertNotIn('>Review</th>', html)
            for note in reviews.values():
                self.assertNotIn(note, html)
            with (Path(tmp) / "purchasing.csv").open(encoding="utf-8-sig") as stream:
                exported = list(csv.DictReader(stream))
            self.assertEqual(exported[1]["Capacitance at bias"], ">=5uF")
            for key, note in reviews.items():
                self.assertEqual(exported[1][key], note)


if __name__ == "__main__":
    unittest.main()
