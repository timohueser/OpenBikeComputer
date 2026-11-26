# obc-dem

`obc-dem` turns Copernicus GLO-30 GeoTIFF into the `.obcd` artifact carried beside a map
([`obct-terrain.md`](../../specs/obct-terrain.md)). It has no native dependency.

```sh
# The tiles a box needs, from the AWS Open Data mirror (~44 MB each).
cargo run --release -p obc-dem -- fetch \
    --bbox 46.48261,8.15034,46.72070,8.46007 --out /tmp/dem

# One .obcd per terrain cell — what a bakery publishes and a catalog names.
cargo run --release -p obc-dem -- bake --sources /tmp/dem \
    --bbox 46.48261,8.15034,46.72070,8.46007 --out cells/

# One .obcd over the whole box — the sidecar a rider carries beside a map.
cargo run --release -p obc-dem -- bake --sources /tmp/dem \
    --bbox 46.48261,8.15034,46.72070,8.46007 --cell-log2 16 --shard grimsel.obcd
```

**`--bbox` is latitude first** (`min_lat,min_lon,max_lat,max_lon`), the opposite of
`obc-pack --bbox`. For an Alpine box both numbers are plausible on either axis, so nothing catches
the mix-up.

`--posting-log2` and `--cell-log2` default to the v1 baked pairing. Both are OBCT header data, so
a different pairing is a re-bake, not a format change.

`fetch` is the only command that uses the network. A bake is a pure function of a tile directory
and a box: identical inputs give byte-identical output, and a digest test pins that contract. A
source void becomes `NODATA`; nothing is inpainted.

Anything derived from GLO-30 must carry the Copernicus credit; `bake` prints it. Its single copy
is the `copernicus-glo-30` entry of [`data/sources.toml`](../../data/sources.toml).

For a published catalog, use `obc bake terrain`, which drives this crate as a library over the
curated coverage (usage: the `bake` recipe in [`tools/justfile`](../../tools/justfile), or
`just --show bake`). [`fixtures/build-map-package.sh terrain`](../../fixtures/build-map-package.sh)
regenerates the two committed terrain sidecars.
