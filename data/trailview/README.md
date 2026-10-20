# TrailView ruleset

The rules that the TrailView ground layers index and that the engine reads. The bake step
`planner/trailview-rules` checks them and bundles them as `rules.json`;
[specs/trailview-tiles.md](../../specs/trailview-tiles.md) defines the bundle and the region ids.

| Path | What it is | Source |
| --- | --- | --- |
| `materials.json` | The ground vocabulary: a material or lithology code is its index | snapshot |
| `regions/`, `packs/`, `styles/` | Rulesets, asset packs and house styles. A file inherits what it does not say from the file it `extends` | snapshot |
| `ecoregions.json` | The RESOLVE ecoregions of Europe, simplified to about 1 km | snapshot |
| `countries.json` | Natural Earth countries and first-level subdivisions | snapshot |
| `overlays/<region>.json` | Look keys of the engine that the prototype does not have | this repository |

## Re-sync

The snapshot files are verbatim copies from the prototype branch. Until the Europe tune of the prototype
ends, change them only with this command, from the repository root:

```sh
S=prototype/trail-view T=data/trailview; git fetch origin archive/trail-view-prototype && \
  git rm -rq --ignore-unmatch $T/{materials.json,regions,packs,styles,ecoregions.json,countries.json} && \
  git checkout origin/archive/trail-view-prototype -- $S/{materials.json,regions,packs,styles} $S/tune/{ecoregions,countries}.json && \
  git mv $S/{materials.json,regions,packs,styles} $S/tune/{ecoregions,countries}.json $T/ && rm -r prototype
```

The command does not touch `overlays/` or this README. Then run the check:

```sh
uv run --locked --group trailview python -m tools.trailview.rules
```

## What bites

- Codes in `materials.json` are indexes in the ground tiles. Append only.
- A region id is the index of the region in the sorted list of `regions/`. A new or removed
  region changes the ids, so the region tiles bake again; the material tiles do not.
- The check does not know the models. A pack that names a model that the engine does not
  have passes the check.
