# Hardware

The electronics of the device, as KiCad 10 projects: schematics, layouts, the project symbol and
footprint library and the 3-D models. `bom.py` makes the parts list with prices. The hardware
design is done by hand; an agent changes `bom.py` and the docs, not the boards.

The firmware pin map is not here. It is in `firmware/obc-fw-nrf54l/src/board.rs`, and it follows
the nRF54LM20 development kit, not the main board. The pins must be ported before the firmware
runs on `OBC_PCB`; the [main board README](projects/OBC_PCB/README.md) has the GPIO map of the
board. Enclosure CAD is not in the repository yet.

## Where to start

| Path | What |
| --- | --- |
| `projects/OBC_PCB/` | The main board. Its [README](projects/OBC_PCB/README.md) has the ERC and DRC commands, the GPIO map and the bring-up rules. It uses `shared/power_management.kicad_sch`. |
| `projects/USB_Baseboard/` | The USB board. It is in the parts list. |
| `projects/USB_Flex/` | The USB flex cable. It is not in the parts list. |
| `projects/OBC_Fit_Test/` | A fit-test schematic with no layout. |
| `libraries/` | `obc.kicad_sym`, `obc.pretty/` and `3dmodels/` ([README](libraries/3dmodels/README.md): units, origin, source of each model). |
| `bom.py` | The combined parts list of the main board and the USB board. |

## Edit the parts data

Open the `.kicad_pro` of `OBC_PCB` or `USB_Baseboard` in KiCad 10. In the Schematic Editor, open
**Edit Symbol Fields** and select **OBC specifications**. Edit ratings, part numbers, supplier
links and prices there. The fields are hidden on the drawing to keep it readable. The schematic
fields are the source of truth; every exported file is generated.

| Field | Format |
| --- | --- |
| Manufacturer / MPN | Exact manufacturer and order code; the same spelling on each instance |
| Mouser / DigiKey | Product page for that order code |
| Alternate MPN / Alternate source | Separate order code, such as another reel size |
| Price source | Supplier page used for the recorded prices |
| Price breaks EUR | Quantity and EUR per part, excluding VAT: `1:0.12;10:0.08;1000:0.04` |
| Price breaks EUR, whole reels | Add an order multiple: `1000@1000:0.03` |
| Price checked | ISO date when the supplier prices were checked |
| Selection notes | Electrical or mechanical checks still required |
| Sourcing notes | Stock, delivery, packaging or quote limits |

For one board's CSV in KiCad, open **Generate Bill of Materials**, select **OBC purchasing** and
the **CSV** format. The preset excludes DNP parts and symbols marked **Exclude from bill of
materials**. It includes shield lids: board exclusion and BOM exclusion are separate flags.

## Run and test

```sh
python3 hardware/bom.py            # writes target/bom/index.html, purchasing.csv and the board CSVs
python3 hardware/bom.py --schematics   # also one PDF per board with all sheets
python3 -m unittest tools.tests.test_hardware_bom
python3 docs/build_docs.py --hardware --check-links   # the site page, as published
```

`bom.py` uses the Python standard library and KiCad 10's `kicad-cli`, from PATH or the macOS
application; `--kicad-cli PATH` and `--output PATH` override the defaults. The tests mock
`kicad-cli`, so they run without KiCad. No `obc` task covers hardware, and no CI job runs ERC
or DRC: run them by hand from the main board README.

The site publishes the report, the CSV files and the schematic PDFs from `develop`. A change
under `hardware/` starts the site build, which installs KiCad and sets `OBC_BUILD_HARDWARE=1`.
See [docs/README.md](../docs/README.md#check-and-preview) for the preview.

## How prices are computed

The two build sizes are 10 and 1,000 devices. Each device has one main board and one USB board.

- A price break uses the total quantity of a part number over both boards, because one purchase
  buys the total.
- A reel price applies only when that quantity is a multiple of the reel size.
- Where no higher break exists, the last eligible break gives a conservative estimate.
- A part with no price stays blank. The sum is then a "known-price subtotal", never a total.
- The report covers schematic components. It leaves out bare PCBs, the USB flex, the display,
  the battery, the SD card, the enclosure, assembly, shipping and VAT.

`tools/tests/test_hardware_bom.py` holds these rules.

## Gotchas

- Two prices for the same manufacturer and MPN stop the export, also across the two boards. Edit
  all instances of an MPN together. When you change an MPN, clear its price fields until you
  have a quote for the new part.
- A price needs a **Price source** and a **Price checked** date, or the export stops.
- `bom.py` selects the KiCad presets **OBC purchasing** and **OBC specifications** by name. Only
  the `.kicad_pro` files of `OBC_PCB` and `USB_Baseboard` define them. A renamed preset breaks
  the export.
- Library and 3-D model paths are relative to the project (`${KIPRJMOD}/../../libraries/...`).
  A moved project folder breaks them. Keep the library tables with the project files.
- The review notes (selection, sourcing, tuning, footprint status) are in `purchasing.csv` and
  not on the HTML page. A test holds this.
- `hardware/.gitignore` ignores `*.csv` and `*.xml`, so an export in the tree is not committed.
- Supplier prices change, and indexed supplier pages lag the live catalogue. Refresh prices and
  check stock before a purchase.
- The report does not qualify the design for production. Check capacitor DC bias and total rail
  capacitance before a release, and review RF tuning and provisional footprints in the notes.

The licence is CERN-OHL-S-2.0 (`LICENSE`); `libraries/` holds the licences of third-party models.
