# Vendored Nordic sEMMC soft-peripheral firmware

`semmc_firmware_v0.1.1.bin` is the **position-independent RISC-V image the FLPR (VPR00) runs to
become an SD/eMMC host controller**. The M33 copies it into the sEMMC carve, points `INITPC` at the
carve base, and then drives the card through the register interface (VRI) the image exposes at
carve base + 16,896. See `src/semmc.rs` for the driver and issue #1158 for the full contract.

## Provenance

| | |
|---|---|
| Upstream | [nrfconnect/sdk-nrfxlib](https://github.com/nrfconnect/sdk-nrfxlib) |
| Path | `softperipheral/sEMMC/include/nrf54l/semmc_firmware_v0.1.1.h` (branch `main`) |
| Version | sEMMC **v0.1.1** (from the image's own metadata header: major 0, minor 1, patch 1) |
| Size | **13,636 B** (the image; it boots into a 15,360 B code region — the tail is zero-init) |
| SHA-256 | `17d2a6c100a4ada0dfbfb870a7693fcadfd15fe6b74d2865d6f0a46f3398f8fe` |
| License | **`LicenseRef-Nordic-5-Clause`** — full text in [`LICENSE.txt`](LICENSE.txt), copied verbatim from the nrfxlib repo root `LICENSE` |
| Copyright | © 2025 Nordic Semiconductor ASA |

The header this is extracted from carries `SPDX-License-Identifier: LicenseRef-Nordic-5-Clause`.
Clause 4 of that license restricts use to a Nordic Semiconductor integrated circuit, and clause 5
forbids reverse engineering — both are satisfied here: the image is redistributed **unmodified**,
runs only on the nRF54L this crate targets, and this repo neither disassembles nor patches it.

## Why it is vendored rather than fetched

- **CI must build offline and reproducibly.** A `build.rs` fetch would make every clean build depend
  on GitHub being reachable and on `main` not moving under us.
- **The same license family already ships linked into this firmware**: `nrf-sdc`/`nrf-mpsl` link
  Nordic's SoftDevice Controller and MPSL binaries under the identical `LicenseRef-Nordic-5-Clause`
  terms. Vendoring one more Nordic binary changes nothing about this crate's distribution posture.
- The image is 13.6 KB and is expected to change roughly never (it is a released soft peripheral).

## Regenerating (byte-for-byte)

From this directory:

```sh
curl -sfL https://raw.githubusercontent.com/nrfconnect/sdk-nrfxlib/main/softperipheral/sEMMC/include/nrf54l/semmc_firmware_v0.1.1.h \
 | python3 -c "import sys,re;open('semmc_firmware_v0.1.1.bin','wb').write(bytes(int(x,16)&0xff for x in re.findall(r'0x[0-9a-fA-F]+',sys.stdin.read().split('semmc_firmware_bin[] = {',1)[1].split('};',1)[0])))"
shasum -a 256 semmc_firmware_v0.1.1.bin   # must match the table above
```

This reproduces the byte-identical image the feasibility bench (#1145, 2026-08-05/06) ran on glass
at 14.7 MB/s read / 8.2 MB/s write.

## Metadata header (first 32 B) — what the carve is derived from

The layout is `softperipheral_metadata_t` (nrfxlib `softperipheral/include/softperipheral_meta.h`,
header version 2). `build.rs` parses exactly these fields out of the vendored image and **asserts**
them against the carve constants, so a blob update that changes the memory footprint fails the
build instead of silently mis-sizing the carve:

| word | field | value | meaning |
|---|---|---|---|
| w0 | `magic` / `header_version` / `comm_id` / `self_boot` | `0xA005` / 2 / 1 / 0 | soft-peripheral image, register-interface (REGIF) comms, **not** self-booting → the host copies it into RAM |
| w2 | `version` | 0.1.1 | |
| w3 | `fw_code_size` × 16 | **15,360** | the code region the host must reserve and zero |
| w3 | `fw_ram_total_size` × 16 | **2,048** | RAM the firmware owns above the code region = 1,536 exec/data + the 512 B VRI |
| w6 | `fw_shared_ram_addr_offset` | **1,536** | VRI offset **within** that RAM region |
| w6 | `fw_shared_ram_size` × 16 | **512** | VRI size |

⇒ VRI at carve base + 15,360 + 1,536 = **16,896**; total carve **17,408 B**.

## Related upstream sources (read, not vendored)

Header-only references used to write the driver — no code from them is copied into this repo:

- `softperipheral/sEMMC/include/nrf_sp_emmc.h` — the VRI register map and `STATUS` bit definitions.
- `softperipheral/include/softperipheral_regif.h` — the `__CSB`/`__ASB`/`__SSB` barrier protocol.
- `softperipheral/include/softperipheral_meta.h` — the metadata header decoded above.
- `softperipheral/sEMMC/src/nrf_semmc.c` — Nordic's own host driver, the shape `src/semmc.rs` follows.
