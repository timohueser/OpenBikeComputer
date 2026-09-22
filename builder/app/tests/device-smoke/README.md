# Device upload smoke

One command sends a map to a connected board over USB, restarts the board, and proves the board
opened what was sent. It exists to catch a transfer that completes and still leaves the wrong bytes,
the wrong revision or an unreadable map on the card.

It needs hardware. There is no mock device behind it: the cable is `usb`'s WebUSB object under the
shipping `openWebUsbLink`, and the reboot is `tools/board.py`. Without a board the command fails.
The phases themselves are `builder/app/test-support/device-smoke/smoke.ts`, and
`smoke.test.ts` drives them against the real flat engine in wasm, which is what CI runs.

## Prerequisites

- A board flashed with the **release** image, running from the ELF you will pass, plugged into the
  host twice: **J4** for the debug probe and **J3** for the product cable
  ([firmware README](../../../../firmware/obc-fw-nrf54l/README.md)).
- A **designated test card** in the board. The run replaces the map object the firmware selects and
  writes nothing else, but that map is gone afterwards. Do not use a card whose map you want.
- `probe-rs` on the path, and no other probe or RTT session open (`obc board doctor`).
- `npm install` in this directory. It installs `usb`, which is a native module, and `tsx`.

## Run

```sh
npm install
npm run smoke -- --replace-map
```

Without `--replace-map` the command prints the store identity and the exact object it would replace,
then stops. Options: `--elf <path>` (default: the release build under
`firmware/obc-fw-nrf54l/target/`), `--probe VID:PID:SERIAL`, `--serial <device serial>`,
`--out <path>`.

The ELF **must** be the image that is running. probe-rs reads it back first and programs nothing
when it already matches; a different ELF is programmed, and a stale one decodes the boot log to
noise.

## What it proves

| Phase | Evidence |
| --- | --- |
| `connect` | The device answers `LIST`; its identity strings come over EP0. The catalog is recorded. |
| `upload` | `PUT` through the shipping `sendMapBytes`, the same call the builder's Send makes. |
| `verify` | `STATUS`, the catalog entry and a whole `GET` all agree with the bytes sent; every other object on the card is still at its revision, length and CRC. |
| `reboot` | `board.py run --preverify` resets the board and streams RTT. The firmware names the object and revision it opened, the box it parsed out of the map header, and the embedded terrain region it mounted. |
| `reverify` | The device comes back on the same store with the same object at the same revision, length and CRC. |

Each phase has its own deadline and fails with the phase that ran out. A refusal, an integrity
mismatch, a stale selection, an unreadable terrain region or a lost object each fail on their own
name rather than on a timeout.

## The map

`apps/obc-sim/assets/grimsel-demo.obcm`, whose sources, producer and digests are pinned in
`fixtures/sources/ride-assistant/grimsel-demo-v18.json`. Nothing is assembled at run time and the
digest is checked before the first byte moves.

Its length is exactly 19,713 whole **card blocks**. The flat store lays an object out over 512-byte
blocks inside its extents, so a payload that is not a multiple of 512 ends inside a block and the
device's last write carries a partial tail; this one does not. It costs no padding: a map carrying
an OBCT v3 **surface** terrain region starts that region on a 512-byte boundary and the region is a
whole number of blocks, so the file ends on one by construction.

This is not the USB packet boundary. A stream record on the wire is four bytes of record prefix, a
sixteen-byte frame header and its payload, batched into writes of tens of kilobytes, so no host
write in this path is a multiple of the 512-byte bulk packet size at any file length.

## The result

A JSON record per run under `.artifacts/device-smoke/`, beside the RTT log: the tool commit, the
firmware ELF and its digest, the identity strings the device reported, the store identity, the map's
digest, the committed object's full reference, what the firmware said at boot, and each phase's
duration. Attach it to the issue that asks for the physical run.

## Limits

The boot evidence is what the release image already prints. It proves the firmware opened that
object at that revision, parsed that map's header, and mounted and parsed the embedded terrain
container off the card. It does not sample an elevation at a pinned coordinate; that would need a
new diagnostic.
