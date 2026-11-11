---
system: device-firmware
kind: shell
purpose: "Builds the nRF54L board image from the device systems."
owns:
  - firmware/obc-fw-nrf54l/**
depends_on:
  - board-inputs
  - card-storage
  - device-link
  - device-runtime
  - firmware-update
  - foundations
  - map-reading
  - navigation
  - rendering-display
test_depends_on: []
status: approved
---

# Device firmware

## What it does

The board image is the program that runs on the device's nRF54L microcontroller. A developer flashes it with `obc flash`, after the bootloader. The image runs on the nRF54LM20 development kit. It moves to the project's own board when the chip is available in the QFN package.

The entry point [`main.rs`](../../firmware/obc-fw-nrf54l/src/main.rs) starts the hardware and runs three tasks: the ride loop, the Bluetooth link and the USB link. The image puts together the device runtime, map reading, navigation, the renderer, card storage, the device link, firmware update and the sensor inputs. Its own glue code holds one RAM block that several large users take in turn ([`arena.rs`](../../firmware/obc-fw-nrf54l/src/arena.rs)), the one task that writes to the card, and the settings in the chip's RRAM.

Board support owns the drivers for the radio, USB, card, panel and sensors. The build script [`build.rs`](../../firmware/obc-fw-nrf54l/build.rs) generates the memory layout and builds the program of the display coprocessor.

## Rules that must hold

| Rule | Check |
| --- | --- |
| A release image fits the recorded RAM, frame and flash budgets. | [`resource_guard.py`](../../firmware/tools/resource_guard.py) against [`resource_baseline.json`](../../firmware/tools/resource_baseline.json) |
| Each user of the shared RAM block fits in it. | The `const` asserts in [`arena.rs`](../../firmware/obc-fw-nrf54l/src/arena.rs) |
| The Bluetooth host has no logging features, because its logs would contain bond keys. | `forbidden_features` in [`dependency_rules.json`](../../firmware/tools/dependency_rules.json), through [`check_dependencies.py`](../../firmware/tools/check_dependencies.py) |

## Decisions

- **One RAM block for users that are never active together.** The render scratch, the navigation planner, the USB write stage, the summit view (Peak View) and the photos take the block in turn. So the board pays for the largest user, not for the sum.
- **One definition of the contract between the two cores.** The build script holds every constant that the main core, the display coprocessor and their linker scripts share. It writes the Rust, C and linker files from it, so a change to one side only is not possible.
