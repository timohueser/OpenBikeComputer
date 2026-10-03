# Hardware BOM

Open either project's `.kicad_pro` in KiCad 10. In the Schematic Editor, open
**Edit Symbol Fields** and select **OBC specifications**. Edit component ratings,
manufacturer part numbers, supplier links and prices there. Save the schematic.
The fields are hidden on the drawing to keep it readable.

For a board CSV, open **Generate Bill of Materials**, select **OBC purchasing**
and the **CSV** format, then click **Export**. This preset excludes DNP parts
and symbols marked **Exclude from bill of materials**. It includes shield lids.
Keep board exclusion separate from BOM exclusion.

For a combined main-board and USB-board report, run from the checkout root:

```sh
python3 hardware/bom.py
open target/bom/index.html
```

Python uses the standard library. Install KiCad 10. The script finds `kicad-cli`
on PATH or in the macOS application. Use `--kicad-cli PATH` to override it.
Use `--output PATH` to select the output directory.
Add `--schematics` to export all schematic sheets as one PDF per board.

The output contains each native board CSV, `purchasing.csv`, and `index.html`.
The combined CSV retains the exported specification fields. Edit the schematic
fields, then run the command again. Generated files are not the source of truth.

| Field | Format |
| --- | --- |
| Manufacturer / MPN | Exact manufacturer and order code; use the same spelling on each instance |
| Mouser / DigiKey | Product page for that order code |
| Alternate MPN / Alternate source | Separate order code, such as another reel size |
| Price source | Supplier page used for the recorded prices |
| Price breaks EUR | Quantity and EUR per part, excluding VAT: `1:0.12;10:0.08;1000:0.04` |
| Price breaks EUR, whole reels | Add an order multiple: `1000@1000:0.03` |
| Price checked | ISO date when the supplier prices were checked |
| Selection notes | Electrical or mechanical checks still required |
| Sourcing notes | Stock, delivery, packaging or quote limits |

The two build sizes are 10 and 1,000 devices. Each device has one main board and
one USB daughterboard. Prices use the total quantity of each MPN across both
boards. A reel price applies only when that quantity is a multiple of the reel
size. Where a higher price break is unavailable, the last recorded eligible
break gives a conservative estimate. Conflicting prices on the same MPN stop
the export. Blank prices remain unknown and are excluded from the subtotal.

Supplier prices and delivery estimates can change. Indexed supplier pages can
lag the live catalogue. Refresh prices and check stock before purchase. Edit
all instances of the same MPN together in KiCad. Clear all price fields when
you change an MPN until you have a quote for the new part.

Check capacitor DC-bias limits and total rail capacitance before release.
Review RF tuning and provisional footprints in the exported notes. This report
does not qualify the circuit or layout for production.

The report covers schematic components. It excludes bare PCBs, the USB flex,
display, battery, SD card, enclosure, assembly, shipping, VAT and extra parts.
The docs site publishes the report, CSV files and schematic PDFs from `develop`.
Hardware changes trigger the site build. To preview the published page locally,
run `python3 docs/build_docs.py --hardware --check-links`. See
[the docs preview instructions](../docs/README.md#check-and-preview).
