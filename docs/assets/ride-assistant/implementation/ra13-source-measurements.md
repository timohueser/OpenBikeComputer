# Swiss landmark encoding measurement

This is a full census of the retained compiled Swiss content. It is not an
estimate from a few photos or a country-wide routing acceptance result.

The normal OSM ingest, explicit approach resolver, landmark join, photo encoder,
and section serializer processed the pinned Swiss PBF and the compiled source
content with network access denied. The report records all input hashes and one
row per QID: [machine-readable census](ra13-switzerland-landmark-census.json).

| Measurement | Result |
| --- | ---: |
| Geographic/type candidates from source compiler | 3,508 |
| Compiled text records | 1,495 |
| Records linked to an OSM identity | 1,134 |
| Records with an explicit mapped approach | 288 |
| Approach counts by configured profile | 278 / 278 / 288 / 288 |
| Licensed photos | 1,119 |
| Raw RGB222 photo bytes | 58,008,960 |
| Compressed photo bytes | 9,375,549 |
| Minimum / median / 95th percentile / maximum photo bytes | 2,415 / 8,137 / 12,550 / 17,347 |
| Directory and record bytes | 137,556 |
| Encoded content bytes after deduplication | 16,280,365 |
| Complete landmark section bytes | 16,417,921 |

The section includes names, text, article notices, compressed images, and full
photo notices. Its size excludes the shared hours pool, routing graph, terrain,
other map sections, and repeated copies in a cell catalog. The encoder validates
photo pixel hashes and the normal compression limits. An approach means that
OSM supplied a permitted entry for a profile. It does not establish connectivity
from an arbitrary rider position.

The geographic compiler count includes the actual Swiss boundary and supported
natural/historical types. It differs from the earlier P17-only study, which used
a country claim and a narrower sample. The retained source compilation produced
669 English, 692 German, 117 French, and 17 Italian records. Omitted or unsupported
sources remain in the source compiler report; they were not replaced with mock
text or images.

## Reproduce

From the repository root, provide the retained pinned PBF and the output of the
normal `obc-bake landmarks` compiler (content.json and sibling RGB222 files):

```sh
cargo build --locked -p obc-pack --example landmark_census
sandbox-exec -p '(version 1) (allow default) (deny network*)' \
  target/debug/examples/landmark_census \
  SOURCE.osm.pbf builder/presets/schema.json CONTENT/content.json census.json
```

The census executable calls `ingest_osm_ways`, `landmark_map::load`, and
`landmark_map::serialize`. It retains routable source ways only during ingest;
it does not build a second graph or implement another image compressor. The
report is a diagnostic artifact, not a map pack. The full raw country capture is
operator input outside the development fixture cache; do not republish it as a
fixture. Shipping regional scenarios use the normal baked map packages.

The example build, example Clippy with `-D warnings`, suite registry, and complete
network-denied census passed. No local shipping image or UI sweep was run for
this measurement. The final integrated simulator and hardware acceptance remain
pending.
