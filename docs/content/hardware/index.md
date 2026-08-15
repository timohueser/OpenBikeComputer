---
title: Hardware
description: The current OpenBikeComputer development hardware and its authoritative references.
---

# Hardware

OpenBikeComputer currently runs on the Nordic **nRF54LM20 development kit** with a
240×320, 64-colour **Sharp LS021B7DD02 reflective memory-LCD**. The board crate owns the
live pin map, sensor wiring, storage transport, build flags and flashing instructions:
[`firmware/obc-fw-nrf54l`](src:firmware/obc-fw-nrf54l/README.md).

The [display protocol](display-protocol/) explains the panel waveform and FLPR-driven
presentation path. Production schematic, PCB and enclosure sources will be documented when
they exist; this page deliberately does not promise or mock up hardware that is not yet in the
repository.
