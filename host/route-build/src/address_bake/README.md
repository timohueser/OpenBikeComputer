# Experimental address baker

Run from the repository root. Use a complete OSM extract with room around the
search region. The output file must not exist.

```sh
python host/route-build/src/address_bake/policy.py /tmp/address-policy.json
cargo build --release -p route-build --features address-bake --bin address-bake
target/release/address-bake REGION.osm.pbf --output addresses.jsonl.zst --default-country ch --policy /tmp/address-policy.json --country-grid /data/country_osm_grid.sql.gz
```

Run the policy export with the Python environment that contains Nominatim.
It exports country ranks and postcode formats from the installed distribution.
The country grid is the static ODbL dataset from the Nominatim distribution.
The baker reads its country polygons without a database. Without the grid,
every record uses `--default-country`. Use that mode only for an extract inside
one country. Country configuration does not establish worldwide output parity.

Build an address package with the existing search builder:

```sh
uv run --with-requirements apps/planner-search/requirements-build.txt python apps/planner-search/build.py addresses.jsonl.zst --component addresses --output /tmp/search/addresses --region REGION --bounds=WEST,SOUTH,EAST,NORTH --countries=de,ch
```

The output contains streets and house numbers. It does not replace the POI
or locality package. Planner preparation keeps Nominatim as its source.
Do not publish this prototype as a replacement before checking lookup output.
Missing relation members omit the affected geometry. The JSON report gives
that count. Interpolation ranges use numeric house numbers. Entrance selection,
geometry repair, and the full tokenizer behavior remain incomplete.

Compare against an existing Nominatim export and its `inputs.json`:

```sh
uv run --with-requirements apps/planner-search/requirements-build.txt python apps/planner-search/address-parity.py addresses.jsonl.zst REFERENCE.jsonl.zst --reference-inputs INPUTS.json --countries=de,ch --output /tmp/address-comparison --require-equivalent
```

The comparison checks source hashes before building both packages. It measures
all reference house records and samples lookups from the reference population.
It reports missing and extra identities, duplicate rows, address fields,
coordinates, forward results, and reverse labels. The coordinate tolerance is
one metre. The report is a measurement, not a release approval.

Run `obc test -p route-build`, then
`cargo nextest run --locked -p route-build --features address-bake` for the
optional baker tests. Run `npm test --prefix apps/planner-search` for comparison
and search tests.
