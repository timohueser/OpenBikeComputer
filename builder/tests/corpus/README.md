# obc-pack test fixtures

Fixtures for the `host/obc-pack` packer tests: hand-authored OSM extracts
whose ingest outcome is known by construction.

- [`tiny/tiny.osm`](tiny/tiny.osm) — exercises every way/area ingest branch. Its
  header comment documents the expected per-element result
  (`ingest::tests::tiny_truth_table`).
- [`poi/poi.osm`](poi/poi.osm) — exercises POI extraction (#422): node + closed-way
  classification, name folding, and the dedup pair
  (`ingest::tests::poi_fixture_end_to_end`).
- [`tiny_split/`](tiny_split/) — `tiny.osm` cut into two overlapping halves for
  the native multi-`.pbf` merge (#920). Their union is exactly `tiny.osm`, so
  ingesting the pair must reproduce ingesting the whole — same features, same
  order, same POIs, same nav graph — while they disagree on purpose about one way
  and one node so the "first source listed wins" tie-break is observable
  (`ingest::tests::merging_two_overlapping_halves_rebuilds_the_whole`,
  `…::the_first_source_carrying_an_id_wins_it`). `tiny_west.osm`'s header comment
  is the map of what each difference is for.
- [`unsorted/unsorted.osm`](unsorted/unsorted.osm) — a way written *before* its
  nodes. The `--bbox` crop's pass 0 stops its node phase at the first way and so
  needs the file type-sorted; this pins the refusal rather than a silently empty
  crop (#910, `ingest::tests::bbox_refuses_an_unsorted_pbf`).
- [`build_corpus.sh`](build_corpus.sh) — converts each to `data/*.osm.pbf` with
  `osmium cat`. The derived `.pbf`s are **also committed** (the tests hard-fail
  without them), so re-run the script and commit the regenerated `.pbf` whenever
  a source `.osm` changes. This script is the last thing in the tree that wants
  `osmium-tool` (`brew install osmium-tool`), and only as an XML→PBF converter —
  nothing here packs a map, and the packer itself needs no osmium at all.

> Historically this directory held a larger validation corpus (monaco, malta,
> Freiburg extracts) used to validate the Rust port against a Python oracle. That
> oracle and its harness have been removed, and so have the port's design notes.

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
