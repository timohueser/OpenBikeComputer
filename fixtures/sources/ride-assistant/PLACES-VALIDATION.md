# Place query validation

The `fixtures.assistant-places` manual suite runs the complete
`host/obc-pack/tests/assistant_places.rs` target. Run it with:

```sh
python3 fixtures/verify-assistant-places.py
```

It reads the pinned `assistant-osm/switzerland.osm.pbf` source from the fixture
cache. The source SHA-256 is
`e6ae53a3cfeb8fbefbab291073e61f0906b03576e773b7403ab9b6d6232a8e88`.
The test uses the repository routing preset and the production ingest, OBCM
serializer, map reader, and place continuation query. It crops the source to
`8.17,46.55,8.38,46.74` and keeps complete source ways at the crop edge.

On 2026-09-15 the query returned 115 distinct service identities across eight
pages. Gletsch station `n420041304` kept its `Q5569509` link, Train category,
and an approach at `(8.361496, 46.561323)` with a nonzero profile mask. Its source
node belongs to service way `w231897254`. Meiringen station `n5007055738` and
water point `n6781569985` have nearby roads but no source node membership in a
routable way. Both kept an unavailable approach. No proximity join was used.

The test packs real service records and the real navigation graph. It uses an
empty render layer and no DEM because it checks place identity and access, not
map rendering or ascent. It does not prove that a visit can connect to the
current rider route. The visit controller must check that in RA05. The integrated
simulator must exercise the same source data through RA06 and RA07. Hardware
acceptance remains pending.

The shared query returns bounded pages in stable order and keeps the original
eligibility time across continuation. Nearby browsing uses those pages. The old
Up Ahead screen consumes its first corridor page; RA07 owns its replacement and
must connect the continuation and closed-selection contract. No coverage map is
available to prove that an installed map has no internal holes. Results therefore
report `coverage_complete: false`, including an empty result.
