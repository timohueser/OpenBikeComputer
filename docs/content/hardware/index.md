---
title: Hardware
description: The current OpenBikeComputer hardware platform and its technical references.
copy: ai
---

# Hardware

OpenBikeComputer uses the Nordic **nRF54LM20 development kit**. It uses a 240 × 320,
64-color **Sharp LS021B7DD02 reflective memory LCD**.

The board crate defines the pin map, sensor connections, storage transport, build flags, and flash procedure:
[`firmware/obc-fw-nrf54l`](src:firmware/obc-fw-nrf54l/README.md).

The [display protocol](display-protocol/) defines the panel waveform and the FLPR scan sequence.

The repository does not contain production schematic, PCB, or enclosure sources.
