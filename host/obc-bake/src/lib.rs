//! `obc-bake` builds the published cell catalog once, centrally, and uploads it.
//! Website and desktop consumers only select, download, and assemble these cells;
//! raw OSM processing stays here as maintainer tooling.
//!
//! A bake takes named Geofabrik regions from [`regions.toml`](../regions.toml), the
//! checked-in schema and skins, and writes one self-contained publish tree:
//!
//! ```text
//! regions.toml ─┐
//!               ├─▶ bake ──▶ <tree>/cells/<band>/<i>/<j>.obcm (+ sidecar)
//! <id>.poly ────┘                    <tree>/regions/<id>/{region.json, boundary.poly}
//!                                    <tree>/{schema.json, skins/<id>.json}
//!                                     │
//!                                     ├─▶ obc-pack catalog ──▶ catalog.json + satellites
//!                                     ├─▶ obc-bake verify ──▶ digests, headers, reader round-trips
//!                                     └─▶ publish ──▶ cells first, root last
//! ```
//!
//! With no positional ids, `bake` processes every entry in `regions.toml`; positional
//! ids select a subset. The output tree is the interface between the long-running,
//! resumable bake and the credentialed publish command.
//!
//! Four properties are structural: curation is reviewable ([`regions`]); skip keys
//! hash source bytes, schema and format ([`cells`]); verification reads the catalog
//! and sampled cells ([`verify`]); publishing uploads and checks every referenced
//! object before replacing the root ([`publish`]).
//!
//! ## Where it runs
//!
//! On a workstation, mostly. A GitHub-hosted runner has 14 GB of RAM and ~14 GB of
//! free disk; the German extract alone is 4.8 GB before the packer allocates
//! anything. `.github/workflows/bake.yml` is one caller of this CLI, sized for small
//! regions and refreshes — the big bakes are an overnight run on a real machine, and
//! nothing here assumes CI.

pub mod cells;
pub mod coverage;
pub mod guard;
pub mod hash;
pub mod presets;
pub mod previews;
pub mod publish;
pub mod regions;
pub mod source;
pub mod verify;
