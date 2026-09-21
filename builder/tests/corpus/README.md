# obc-pack test fixtures

Hand-authored OSM extracts for the `host/obc-pack` packer tests, whose ingest outcome is known by
construction.

| Fixture | What it pins | Test |
| --- | --- | --- |
| [`tiny/tiny.osm`](tiny/tiny.osm) | Every way and area ingest branch. Its header comment is the expected per-element result. | `ingest::tests::tiny_truth_table` |
| [`poi/poi.osm`](poi/poi.osm) | POI extraction: node and closed-way classification, name folding, and the dedup pair. | `ingest::tests::poi_fixture_end_to_end` |
| [`tiny_split/`](tiny_split/) | `tiny.osm` cut into two overlapping halves for the native multi-`.pbf` merge. Their union is exactly `tiny.osm`, so ingesting the pair must reproduce ingesting the whole. They disagree on purpose about one way and one node, which makes the "first source listed wins" tie-break observable; `tiny_west.osm`'s header comment maps each difference. | `ingest::tests::merging_two_overlapping_halves_rebuilds_the_whole`, `…::the_first_source_carrying_an_id_wins_it` |
| [`unsorted/unsorted.osm`](unsorted/unsorted.osm) | A way written *before* its nodes. The `--bbox` crop's pass 0 stops its node phase at the first way, so it needs a type-sorted file; this pins the refusal rather than a silently empty crop. | `ingest::tests::bbox_refuses_an_unsorted_pbf` |

`tiny.osm` way 106 also pins the closed-way rule: a closed way is a polygon only when it carries
`area=yes` or an area tag, and never when it carries `area=no`. Relations are always areas. So a
closed `highway=residential` loop is a line and not also a filled blob.

## Rebuild the `.pbf` files

```sh
./build_corpus.sh
```

It converts each source to `data/*.osm.pbf` with `osmium cat`. **The derived `.pbf` files are
committed too, and the tests hard-fail without them**, so re-run the script and commit the
regenerated `.pbf` whenever a source `.osm` changes.

This script is the last thing in the tree that wants `osmium-tool` (`brew install osmium-tool`),
and only as an XML-to-PBF converter. Nothing here packs a map, and the packer itself needs no
osmium.
