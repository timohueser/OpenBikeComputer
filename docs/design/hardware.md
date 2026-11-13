---
system: hardware
kind: system
area: Hardware
purpose: "Holds the board's schematics, layouts and component models, and makes its parts list with prices."
owns:
  - hardware/**
depends_on: []
test_depends_on: []
status: approved
---

# Hardware

## What it does

The hardware folder holds the KiCad 10 projects of the electronics: the main board ([`OBC_PCB`](../../hardware/projects/OBC_PCB/README.md)), the USB board (`USB_Baseboard`) and the USB flex cable (`USB_Flex`). The project symbols, footprints and 3-D models are in `libraries/`. Each component has its part number, supplier links and price breaks in hidden fields of the schematic.

The script [`bom.py`](../../hardware/bom.py) exports the parts list of the main board and the USB board with `kicad-cli`. It adds up the quantity of each part over both boards, and gives a price for 10 and for 1,000 devices. The website publishes this report, the CSV files and the schematic PDFs. The [hardware README](../../hardware/README.md) tells how to edit the fields.

The board image keeps the pin assignments of the firmware in [`board.rs`](../../firmware/obc-fw-nrf54l/src/board.rs). They follow the development kit, not the main board.

## Rules that must hold

| Rule | Check |
| --- | --- |
| Two prices for the same part number stop the export, and a price needs a source and a check date. | `test_conflicting_or_unattributed_prices_stop_export` in [`test_hardware_bom.py`](../../tools/tests/test_hardware_bom.py) |
| A reel price applies only to a quantity of whole reels. | `test_reel_discount_requires_whole_reels` in [`test_hardware_bom.py`](../../tools/tests/test_hardware_bom.py) |
| A part shared by the two boards gets the price break of the total quantity. | `test_shared_parts_reach_break_with_combined_board_quantity` in [`test_hardware_bom.py`](../../tools/tests/test_hardware_bom.py) |
| A part with no price is blank, and the report does not call a partial sum a total. | `test_unknown_quote_is_blank_and_report_never_calls_subtotal_total` in [`test_hardware_bom.py`](../../tools/tests/test_hardware_bom.py) |
