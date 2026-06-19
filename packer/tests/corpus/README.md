# obc-pack test fixtures

Fixtures for the `firmware/obc-pack` packer tests. The one the automated tests
need is **`tiny`** — a hand-authored OSM extract that exercises every ingest
branch by construction.

- [`tiny/tiny.osm`](tiny/tiny.osm) — the committed XML source of truth. Its header
  comment documents the expected per-element ingest result.
- [`build_corpus.sh`](build_corpus.sh) — converts it to `data/tiny.osm.pbf` with
  `osmium cat` (the `.pbf` is git-ignored). The Rust
  `ingest::tests::tiny_truth_table` test reads that file (and skips if absent).

> Historically this directory held a larger validation corpus (monaco, malta,
> Freiburg extracts) used to validate the Rust port against a Python oracle. That
> oracle and its harness have been removed; the port's design notes live in
> [`../../docs/`](../../docs/).

## Known intentional divergence (a bug we do NOT replicate)

`tiny.osm` pins the one place the packer deliberately differs from the old Python
oracle. The oracle double-emitted **closed line-ways**: osmium's `AreaManager`
built a polygon for *every* closed way except `area=no`/multipolygon members, and
ingest emitted it whenever the tags matched a configured style — even line styles
like `highway=residential` — while the way was *also* emitted as a line. So a
closed residential loop became **both** a line and a filled blob. `obc-pack`
classifies a closed way as a polygon iff `area=yes` or it carries an area tag (and
not `area=no`); relations are always areas. `tiny.osm` way 106 pins this (a closed
`highway=residential` → line only, no blob).
