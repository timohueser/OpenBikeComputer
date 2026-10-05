# Search baker

Run `obc planner prepare` from the checkout to build addresses, POIs, and
localities from the recipe's OSM snapshot. See the
[planner README](../../builder/app/src/components/planner/README.md).
The pipeline verifies the input hash and builds the search packages and place tiles.
It needs no Nominatim database, Photon export, or PostgreSQL installation.

For a direct bake, use a complete OSM extract with room around the search region.
Export the static country data from the pinned archive in
[`tools/planner_sources.py`](../../tools/planner_sources.py):

```sh
uv run --with PyYAML==6.0.2 python host/obc-search-bake/policy.py COUNTRY_DATA.whl /tmp/country-data
cargo build --locked --release -p obc-search-bake
target/release/search-bake REGION.osm.pbf --output search.jsonl.zst --default-country ch --policy /tmp/country-data/policy.json --country-grid /tmp/country-data/country_osm_grid.sql.gz
```

The output file must not exist. The baker publishes it only after all records
are written. The JSON report includes the count of incomplete geometries.
Missing relation members omit the affected geometry.

Country ranks, postcode formats, and the ODbL country grid come from the pinned
Nominatim distribution. The archive is read as data; no Nominatim code runs.
Current OSM country boundaries take precedence over the grid. Missing coverage
uses `--default-country`. A house takes its country from its parent street or place.

The device and search bakers use [`obc-places`](../obc-places/) for shared POI
categories and coordinates. Search retains names, aliases, contact details, and
opening hours. Locality nodes retain the extent of linked administrative areas.
The planner also searches categories that the device does not display.

Compare addresses against an existing reference export and its `inputs.json`:

```sh
uv run --with-requirements apps/planner-search/requirements-build.txt python apps/planner-search/address-parity.py search.jsonl.zst REFERENCE.jsonl.zst --reference-inputs INPUTS.json --countries=de,ch --time-zone=Europe/Berlin --output /tmp/address-comparison --require-equivalent
```

The comparison verifies source hashes and checks address fields, coordinates,
forward results, and reverse labels. Interpolation supports numeric ranges.
Entrance selection, geometry repair, and tokenization can differ from Nominatim.

Run `obc test -p obc-search-bake` and
`cargo clippy -p obc-search-bake --all-targets -- -D warnings`.
The tests compare device and search POI identities, categories, and coordinates.
Run `npm test --prefix apps/planner-search` for search tests.
