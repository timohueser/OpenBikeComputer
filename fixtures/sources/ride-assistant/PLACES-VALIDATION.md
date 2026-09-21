# Place query validation

The `fixtures.assistant-places` manual suite runs the whole
`host/obc-pack/tests/assistant_places.rs` target:

```sh
python3 fixtures/verify-assistant-places.py
```

It reads the pinned `assistant-osm/switzerland.osm.pbf` from the fixture cache and crops it to
`8.17,46.55,8.38,46.74`, keeping complete source ways at the crop edge. The test uses the
repository routing preset and the production ingest, OBCM serializer, map reader and place
continuation query, so it checks place identity and access rather than rendering: it packs real
service records and the real navigation graph with an empty render layer and no DEM.

A place gets an approach only through its source node's membership in a routable way. **No
proximity join is used**, so a place with a road nearby but no such membership keeps an
unavailable approach.

The shared query returns bounded pages in stable order and keeps the original eligibility time
across continuation. No coverage map can prove that an installed map has no internal holes, so
results report `coverage_complete: false`, including for an empty result.
