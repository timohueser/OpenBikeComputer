# Search baker

`obc data update` builds addresses, POIs, and localities from the OSM of the region. See the
[planner README](../../builder/web/src/components/planner/README.md).
The pipeline verifies the input hash and builds the search packages and place tiles.
It needs no Nominatim database, Photon export, or PostgreSQL installation.

The step `planner/search/policy` runs `policy.py`. It exports the static country data from the
`nominatim-country-data` archive of [`data/sources.toml`](../../data/sources.toml). The step
`planner/search/dump` bakes `search.jsonl.zst`. Its report includes the count of incomplete
geometries. Missing relation members omit the affected geometry.

Country ranks, postcode formats, and the ODbL country grid come from the pinned
Nominatim distribution. The archive is read as data; no Nominatim code runs.
Current OSM country boundaries take precedence over the grid. Missing coverage
uses the `country` option of the step. A house takes its country from its parent street or place.

The device and search bakers use [`obc-places`](../obc-places/) for shared POI
categories and coordinates. Search retains names, aliases, contact details, and
opening hours. Locality nodes retain the extent of linked administrative areas.
The planner also searches categories that the device does not display.

## Test

Run `obc test -p obc-search-bake` and
`cargo clippy -p obc-search-bake --all-targets -- -D warnings`.
The tests compare device and search POI identities, categories, and coordinates.
Run `npm test --prefix planner/search` for search tests.

## Pitfalls

Address interpolation supports numeric ranges.
Entrance selection, geometry repair, and tokenization can differ from Nominatim.
