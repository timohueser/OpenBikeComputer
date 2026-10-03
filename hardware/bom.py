#!/usr/bin/env python3
"""Export KiCad BOMs and cost one main board plus one USB board per device."""

import argparse
import csv
from decimal import Decimal
from html import escape
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent
BOARDS = ("OBC_PCB", "USB_Baseboard")
TIERS = (10, 1000)
KICAD_CLI = shutil.which("kicad-cli") or "/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli"


def unit_price(breaks, quantity):
    eligible = []
    for item in filter(None, breaks.split(";")):
        threshold, price = item.split(":")
        minimum, _, multiple = threshold.partition("@")
        minimum, multiple, price = int(minimum), int(multiple or 1), Decimal(price)
        if minimum < 1 or multiple < 1 or not price.is_finite() or price <= 0:
            raise ValueError(f"Invalid price break: {item}")
        if minimum <= quantity and quantity % multiple == 0:
            eligible.append((minimum, price))
    return max(eligible)[1] if eligible else None


def cost_rows(rows):
    quantities = {}
    prices = {}
    for row in rows:
        key = (row["Manufacturer"], row["MPN"])
        if not key[1]:
            continue
        quantities[key] = quantities.get(key, 0) + int(row["Qty"])
        quote = (row["Price source"], row["Price breaks EUR"], row["Price checked"])
        if key in prices and prices[key] != quote:
            raise ValueError(f"Conflicting prices for {key[1]}; edit all instances in KiCad")
        prices[key] = quote
    for row in rows:
        key = (row["Manufacturer"], row["MPN"])
        qty = int(row["Qty"])
        for count in TIERS:
            row[f"Qty / {count} devices"] = str(qty * count)
            purchase_qty = quantities.get(key, qty) * count
            price = unit_price(row["Price breaks EUR"], purchase_qty) if key[1] else None
            if price is not None and not all(row[f] for f in ("Price source", "Price checked")):
                raise ValueError(f"Price lacks source or check date: {key[1]}")
            row[f"Unit EUR / {count} devices"] = str(price) if price is not None else ""
            row[f"Line EUR / {count} devices"] = f"{price * qty * count:.2f}" if price is not None else ""
    return rows


def report_html(rows, schematics=False):
    parts = ['<section class="hardware-bom">']
    if schematics:
        parts.extend(['<h2 id="schematics">Schematics</h2>',
                      '<p>Each PDF contains all sheets for that board. Use the sheet links to navigate.</p>',
                      '<ul><li><a href="OBC_PCB.pdf" download>Main board schematic (PDF)</a></li>',
                      '<li><a href="USB_Baseboard.pdf" download>USB daughterboard schematic (PDF)</a></li></ul>'])
    parts.extend(['<h2 id="components">Components and prices</h2>',
             '<p><a href="purchasing.csv" download>Download combined purchasing CSV</a> · ',
             '<a href="OBC_PCB.csv" download>Main board CSV</a> · ',
             '<a href="USB_Baseboard.csv" download>USB daughterboard CSV</a></p>',
             '<p>One main board and one USB daughterboard per device. Bare PCBs, flex, display,',
             'battery, SD card, enclosure, assembly, shipping and VAT are excluded. DNP parts are excluded.</p>',
             '<p>Prices are recorded estimates, not live quotes. Recheck stock and delivery before ordering.',
             'Price breaks use the combined MPN quantity across both boards. Reel prices apply only to whole reels.',
             'The last recorded cut-tape break is used when no higher eligible price is recorded.',
             'No extra parts, reel fees or assembly losses are included. Blank prices are unknown.</p>'])
    for count in TIERS:
        field = f"Line EUR / {count} devices"
        missing = sum(not row[field] for row in rows)
        total = sum((Decimal(row[field]) for row in rows if row[field]), Decimal(0))
        label = "Known-price subtotal" if missing else "Estimated parts total"
        parts.append(f'<p><strong>{count:,} devices: {label} €{total:,.2f}</strong> '
                     f'(€{total / count:,.2f}/device); {missing} unpriced rows.</p>')
    columns = ["Board / references", "Qty", "Part", "Specifications", "10 devices",
               "1,000 devices", "Suppliers"]
    parts.append('<div class="table-wrap" tabindex="0" role="region" aria-label="Component bill of materials"><table><thead><tr>' +
                 ''.join(f'<th scope="col">{escape(c)}</th>' for c in columns) +
                 '</tr></thead><tbody>')
    for row in rows:
        board = {"OBC_PCB": "Main board", "USB_Baseboard": "USB daughterboard"}.get(row["Board"], row["Board"])
        cells = [escape(board) + '<br>' + escape(row["Reference"].replace(',', ', ')), row["Qty"],
                 escape(row["Value"]) + '<br>' + escape(row["MPN"]) +
                 '<small>' + escape(row["Manufacturer"]) + '</small>']
        specs = ["Footprint", "Voltage", "Tolerance", "Power", "Dielectric", "Rated Current",
                 "Saturation Current", "DCR", "Capacitance at bias", "Capacitance budget",
                 "Load Capacitance", "Frequency tolerance", "Initial tolerance", "ESR"]
        cells.append('<br>'.join(escape(f'{key}: {row[key]}') for key in specs if row.get(key)))
        for count in TIERS:
            price, total = (row[f'{kind} / {count} devices'] for kind in ("Unit EUR", "Line EUR"))
            cells.append(f'{row[f"Qty / {count} devices"]} parts<br>' +
                         (f'€{price} each<br><strong>€{total}</strong>' if price else 'Price unknown'))
        links = []
        for key in ("Mouser", "DigiKey", "Price source", "Alternate source"):
            url = row.get(key, "")
            if url.startswith("https://"):
                links.append(f'<a href="{escape(url, quote=True)}">{key}</a>')
        cells.append('<br>'.join(links) + '<small>' + escape(row.get("Price checked", "")) + '</small>')
        parts.append('<tr>' + ''.join(f'<td>{cell}</td>' for cell in cells) + '</tr>')
    parts.append('</tbody></table></div></section>')
    return '\n'.join(parts)


def write_report(rows, output, schematics=False):
    columns = list(dict.fromkeys(key for row in rows for key in row))
    with (output / "purchasing.csv").open("w", newline="", encoding="utf-8-sig") as stream:
        writer = csv.DictWriter(stream, fieldnames=columns)
        writer.writeheader()
        writer.writerows({key: row.get(key, "") for key in columns} for row in rows)
    content = report_html(rows, schematics)
    page = ('<!doctype html><html lang="en"><head><meta charset="utf-8">'
            '<meta name="viewport" content="width=device-width,initial-scale=1">'
            '<title>OBC bill of materials</title>'
            '<style>body{font:15px system-ui;margin:2rem;color:#222}'
            'table{border-collapse:collapse;width:100%;min-width:1000px;table-layout:fixed;font-size:13px}'
            'th,td{padding:.6rem;border:1px solid #ccc;text-align:left;vertical-align:top}'
            'th{background:#eee}td{overflow-wrap:anywhere}a{color:#155e75}'
            '.table-wrap{overflow:auto}small{display:block;color:#555;margin-top:.3rem}</style>'
            '</head><body><h1>OBC bill of materials</h1>' + content + '</body></html>')
    (output / "index.html").write_text(page, encoding="utf-8")
    return content


def export(output, kicad_cli=KICAD_CLI, schematics=False):
    rows = []
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="obc-bom-") as temporary:
        for board in BOARDS:
            schematic = ROOT / "projects" / board / f"{board}.kicad_sch"
            exported = Path(temporary) / f"{board}.csv"
            subprocess.run([kicad_cli, "sch", "export", "bom", "--preset", "OBC purchasing",
                            "--format-preset", "CSV", "--output", str(exported), str(schematic)],
                           check=True)
            with exported.open(encoding="utf-8-sig", newline="") as stream:
                rows.extend(dict(row, Board=board) for row in csv.DictReader(stream))
            shutil.copyfile(exported, output / exported.name)
            if schematics:
                subprocess.run([kicad_cli, "sch", "export", "pdf", "--no-background-color",
                                "--output", str(output / f"{board}.pdf"), str(schematic)], check=True)
    return write_report(cost_rows(rows), output, schematics)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT.parent / "target" / "bom")
    parser.add_argument("--kicad-cli", default=KICAD_CLI)
    parser.add_argument("--schematics", action="store_true", help="Also export all schematic sheets as PDFs")
    args = parser.parse_args()
    export(args.output, args.kicad_cli, args.schematics)
    print(f"BOM: {args.output.resolve() / 'index.html'}")


if __name__ == "__main__":
    main()
