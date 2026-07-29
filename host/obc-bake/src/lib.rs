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
//!               ├─▶ bake ──▶ <tree>/regions/<id>/<preset>.obcm (+ sidecar)
//! presets/*.json┘             <tree>/presets/<preset>.json
//!                              │
//!                              ├─▶ obc-pack catalog ──▶ catalog.json
//!                              └─▶ publish ──▶ artifacts first, manifest last
//! ```
//!
//! Four properties are the point of the whole thing, and each has a module:
//!
//! - **Curation is reviewable.** [`regions`] — the shelf is a checked-in file where
//!   adding coverage is one line and the comments explain the choices.
//! - **A published artifact is readable.** [`verify`] — every artifact is opened
//!   with the real `obc-reader` and walked whole before it is allowed into the tree.
//!   Not a header sniff; a full decode of every feature in every chunk.
//! - **Re-running is cheap and honest.** [`bake`] — the skip decision is a hash of
//!   the extract, the preset and the format version, never a timestamp.
//! - **The catalog is never half-published.** [`publish`] — artifacts first, their
//!   presence re-checked at the destination, manifest last as one object swap.
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
pub mod guard;
pub mod hash;
pub mod presets;
pub mod publish;
pub mod regions;
pub mod source;
pub mod verify;
