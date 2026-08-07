# Vendored `embassy-usb-synopsys-otg` — provenance

This is the **Synopsys DWC2 OTG device driver** the nRF54LM20's USBHS runs on. `embassy-nrf`'s
`usb::Driver` is a thin wrapper over it (`embassy-nrf-0.11.0/src/usb/usbhs.rs` — it supplies the
register block, the FIFO depth and the PHY type and forwards every trait call), so this crate is
where the OUT-endpoint transfer arming lives, and therefore where the upload throughput ceiling
sits (issue #1173).

The board crate redirects `embassy-nrf`'s dependency here with a `[patch.crates-io]` entry in
`firmware/obc-fw-nrf54l/Cargo.toml`. Nothing else in the repo resolves it: the workspace root and
`obc-boot` have no USB at all.

## Provenance

| | |
|---|---|
| Upstream | [embassy-rs/embassy](https://github.com/embassy-rs/embassy), `embassy-usb-synopsys-otg` |
| Version | **0.4.0** (2026-05-28), the exact version `Cargo.lock` resolved before vendoring |
| crates.io checksum | `6efd0c0e0b21bcf91b475b8fa47347c1df32c9a99728ae1e1b0e6625862dfd1a` |
| Upstream commit | `664d4ead36bb24a63955ca649bcec66c6e70bf6d` (from the published crate's `.cargo_vcs_info.json`) |
| License | **MIT OR Apache-2.0** — both texts verbatim in [`LICENSE-MIT`](LICENSE-MIT) / [`LICENSE-APACHE`](LICENSE-APACHE) |
| Copyright | © Embassy project contributors |

`src/`, `README.md` and `CHANGELOG.md` are the published crate's files. `Cargo.toml` is the
upstream `Cargo.toml.orig` with the three `path = "../embassy-*"` attributes dropped (they point
into the embassy monorepo, which is not here) and an empty `[workspace]` table added so the board
crate's workspace does not adopt this directory as a member. It deliberately carries **no**
`publish = false`: `about.toml` sets `private.ignore = true`, so that field would drop the crate
out of `THIRD-PARTY.md` — and this code ships inside `UPDATE.BIN`, where embassy's MIT/Apache-2.0
terms have to travel with it like every other third-party crate in the image.

## Regenerating the pristine copy

The vendored tree is byte-identical to the crates.io `.crate` for 0.4.0 apart from the `Cargo.toml`
edits above and the two license files, which crates.io does not package. To re-derive it:

```sh
cargo download embassy-usb-synopsys-otg==0.4.0   # or fetch from a warm ~/.cargo/registry/src
```

The first commit of the PR that introduced this directory is the pristine copy, so
`git diff <that commit> -- firmware/obc-fw-nrf54l/vendor/embassy-usb-synopsys-otg` is always the
complete local fork.

## The local fork: multi-packet bulk OUT (issue #1173)

**One change, one endpoint.** Upstream `EndpointOut::read` arms exactly one max packet
(`DOEPTSIZ.PKTCNT = 1`, `XFRSIZ = max_packet_size`) and re-arms it only after the application task
has copied the packet out and cleared NAK. The endpoint therefore NAKs from the moment a packet
lands until the executor gets round to polling the reader — ISR → waker → executor scan → poll →
copy → CNAK per 512 B — which measured ~342 µs/packet on glass (2026-08-07), of which ~240 µs was
that serialization. Map uploads ran at 1416–1459 kB/s on both hosts as a result.

The fork adds an **opt-in per-endpoint burst** to that arming:

* `Config::out_burst_endpoints` — a bitmask of OUT endpoint *indices* that burst.
* `Config::out_burst_packets` — how many max packets those endpoints arm at once.

For a bursting endpoint the driver arms `PKTCNT = N` / `XFRSIZ = N × mps`, gives it `N × mps` of
staging buffer instead of one packet, and sizes its share of the shared RX FIFO for `N` packets.
The interrupt handler **appends** each arriving packet to that buffer instead of overwriting it, and
`read` drains everything staged in one pass. The endpoint stops NAKing between packets, so the core
keeps absorbing the stream while the CPU is busy folding CRC and writing the card.

Everything else — EP0, the control pipe, every IN endpoint, and any OUT endpoint not named in the
mask — keeps the upstream `PKTCNT = 1` behaviour byte for byte. `out_burst_endpoints = 0` (the
default) is upstream.

Details, including why `read` publishes per packet rather than per completed transfer, are in the
`// ===== OpenBikeComputer fork =====` comments in `src/lib.rs`.

## Upstreaming

The shape is deliberately upstreamable: a `Config` field, no API break, no behaviour change at the
default. If it goes to embassy, this directory and the `[patch.crates-io]` entry both go away.
