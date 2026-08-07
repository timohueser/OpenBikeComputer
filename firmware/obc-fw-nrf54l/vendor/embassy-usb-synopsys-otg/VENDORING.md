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
crate's workspace does not adopt this directory as a member.

## Regenerating the pristine copy

The vendored tree is byte-identical to the crates.io `.crate` for 0.4.0 apart from the `Cargo.toml`
edits above and the two license files, which crates.io does not package. To re-derive it:

```sh
cargo download embassy-usb-synopsys-otg==0.4.0   # or fetch from a warm ~/.cargo/registry/src
```

The first commit of the PR that introduced this directory is the pristine copy, so
`git diff <that commit> -- firmware/obc-fw-nrf54l/vendor/embassy-usb-synopsys-otg` is always the
complete local fork.

## The local fork

None yet — this commit is the pristine vendored copy, so that the functional change that follows is
reviewable on its own.
