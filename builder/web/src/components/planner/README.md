# Route planner

The public planner at `/plan/` uses one active release for maps, routing, and
search.

## Prepare a region

`obc data` builds the planner layers with the device layers. `obc data setup` lists the tools
that the steps need on this machine, such as Java 21 for the basemap.

```sh
obc data update local --region baden-wuerttemberg-switzerland
obc data serve
```

`obc data update` publishes a target, and the VPS follower of
[`tools/vps/README.md`](../../../../../tools/vps/README.md) runs the planner services of `prod`.
The [release contract](../../../../../specs/planner-release.md) defines the files. The
[runtime target](../../../../../specs/obc-data.md#service-runtimes) of the services is in
[`data/planner-runtime.toml`](../../../../../data/planner-runtime.toml).

## Client configuration

The planner reads the `active`
[catalogue](../../../../../specs/planner-release.md#catalogue) entry at page load.
`VITE_PLANNER_CATALOG` names another catalogue, such as the one that `obc data serve`
prints. The map builder then reads its device catalogue too. `VITE_PLANNER_CONFIG`
replaces the entry with one entry. `VITE_CATALOG_URL` gives the map builder another
device catalogue.

Basemap zooms are 0–14; terrain zooms are 0–12. Browser contours use terrain
neighbours. Highlighted places use zoom 11.

Extract a smaller map archive with bounds inside its source coverage:

```sh
python3 -m tools.planner_maps compact /srv/planner/bw/maps/terrain.pmtiles \
  /srv/planner/terrain.pmtiles --bbox=7.8,47.9,8.1,48.2 --terrain
```

Omit `--terrain` for a basemap. The command uses `uv` with pinned dependencies.
A map cutout does not change routing or search coverage.
Use `--no-recompress` to crop compressed terrain without encoding it again.

## Checks

From `builder/web`:

```sh
npx vitest run src/lib/planner/ src/components/planner/
npm run check
```

Type checking needs generated WASM.
Sunlight benchmark:

```sh
node ../../tools/planner_sun_bench.mjs http://127.0.0.1:4175
```

[Sunlight index](../../../../../specs/planner-sun-tiles.md).
See the [search README](../../../../../planner/search/README.md) for its
code and real-data suites. iOS rendering and offline downloads have separate
validation.
