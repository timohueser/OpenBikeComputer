# obc-pack test fixtures

Fixtures for the `firmware/obc-pack` packer tests: hand-authored OSM extracts
whose ingest outcome is known by construction.

- [`tiny/tiny.osm`](tiny/tiny.osm) — exercises every way/area ingest branch. Its
  header comment documents the expected per-element result
  (`ingest::tests::tiny_truth_table`).
- [`poi/poi.osm`](poi/poi.osm) — exercises POI extraction (#422): node + closed-way
  classification, name folding, and the dedup pair
  (`ingest::tests::poi_fixture_end_to_end`).
- [`build_corpus.sh`](build_corpus.sh) — converts each to `data/*.osm.pbf` with
  `osmium cat`. The derived `.pbf`s are **also committed** (the tests hard-fail
  without them), so re-run the script and commit the regenerated `.pbf` whenever
  a source `.osm` changes.

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
