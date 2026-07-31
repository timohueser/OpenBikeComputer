//! `obc-bake` — the bakery: build the curated regions once, centrally, and publish
//! them as static files.
//!
//! The hosted builder (#894) has no backend. A rider picks a region and downloads a
//! `.obcm` that was packed hours or weeks earlier on someone else's machine, listed
//! in a catalog manifest ([`OBCC_Spec.md`](../../../specs/OBCC_Spec.md)). This crate
//! is the thing on the other end of that: it turns
//! [`regions.toml`](../regions.toml) × `builder/presets/` into a bake tree, and the
//! bake tree into objects in a bucket.
//!
//! ```text
//! regions.toml ─┐
//!               ├─▶ bake ──▶ <tree>/regions/<id>/<schema>.obcm (+ sidecar)
//! schema.json ──┘             <tree>/presets/<schema>.json
//!                              │
//!                              ├─▶ obc-pack catalog ──▶ catalog.json
//!                              └─▶ publish ──▶ artifacts first, manifest last
//! ```
//!
//! `builder/presets/` is one **schema** plus its **skins** since #1036 ([`presets`]):
//! the schema is the config everything is baked with, and a skin is presentation
//! stamped onto an assembled map's style table. The v1 path above therefore bakes one
//! artifact per region rather than a (region × preset) matrix — a whole-region `.obcm`
//! has its styling in its bytes, so it can only ever carry the schema's own look.
//!
//! Four properties are the point of the whole thing, and each has a module:
//!
//! - **Curation is reviewable.** [`regions`] — the shelf is a checked-in file where
//!   adding coverage is one line and the comments explain the choices.
//! - **A published artifact is readable.** [`verify`] — every artifact is opened
//!   with the real `obc-reader` and walked whole before it is allowed into the tree.
//!   Not a header sniff; a full decode of every feature in every chunk.
//! - **Re-running is cheap and honest.** [`bake`] — the skip decision is a hash of
//!   the extract, the schema and the format version, never a timestamp.
//! - **The catalog is never half-published.** [`publish`] — artifacts first, their
//!   presence re-checked at the destination, manifest last as one object swap.
//!
//! ## The cell path (#1016 P2)
//!
//! The same four properties, one unit smaller. `obc-bake bake --cells <region…>`
//! resolves each curated region to the **grid cells** its coverage polygon touches
//! ([`coverage`]), cuts exactly those from exactly the extracts that can complete
//! them ([`cells`]), and writes the tree `obc-pack catalog --v2` turns into an
//! [`OBCC_Spec.md`](../../../specs/OBCC_Spec.md) §11 cell catalog. Regions stop being
//! artifacts and become selections; two regions that share ground share cells, and
//! the store pays for that ground once.
//!
//! ```text
//! regions.toml ─┐
//!               ├─▶ bake --cells ──▶ <tree>/cells/<band>/<i>/<j>.obcm (+ sidecar)
//! <id>.poly ────┘                    <tree>/regions/<id>/{region.json, boundary.poly}
//!                                    <tree>/{schema.json, skins/<id>.json}
//!                                     │
//!                                     ├─▶ obc-pack catalog --v2 ──▶ catalog.json + satellites
//!                                     ├─▶ obc-bake verify ──▶ digests, headers, reader round-trips
//!                                     └─▶ publish --v2 ──▶ cells first, root last
//! ```
//!
//! And one more that is not a property of the code but of the operation:
//! **failures are loud** ([`bake::RunSummary`]). A region that quietly did not bake
//! is indistinguishable, to a user, from a region deliberately not offered.
//!
//! ## Where it runs
//!
//! On a workstation, mostly. A GitHub-hosted runner has 14 GB of RAM and ~14 GB of
//! free disk; the German extract alone is 4.8 GB before the packer allocates
//! anything. `.github/workflows/bake.yml` is one caller of this CLI, sized for small
//! regions and refreshes — the big bakes are an overnight run on a real machine, and
//! nothing here assumes CI.

pub mod bake;
pub mod cells;
pub mod coverage;
pub mod guard;
pub mod hash;
pub mod presets;
pub mod publish;
pub mod regions;
pub mod source;
pub mod verify;
