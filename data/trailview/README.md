# TrailView ruleset

The rules that the TrailView ground layers index and that the engine reads. The bake step
`planner/trailview-rules` checks them and bundles them as `rules.json`;
[specs/trailview-tiles.md](../../specs/trailview-tiles.md) defines the bundle and the region ids.

| Path | What it is |
| --- | --- |
| `materials.json` | The ground vocabulary: a material or lithology code is its index |
| `regions/`, `packs/`, `styles/` | Rulesets, asset packs and house styles. A file inherits what it does not say from the file it `extends` |
| `ecoregions.json` | The RESOLVE ecoregions of Europe, simplified to about 1 km |
| `countries.json` | Natural Earth countries and first-level subdivisions: the places of a value by place |
| `kinds.json` | The feature kinds of `planner/trailview-features`: [specs/trailview-features.md](../../specs/trailview-features.md) |
| `ground.json` | The ground sources of the material and the lithology, in the order they apply: [specs/trailview-tiles.md](../../specs/trailview-tiles.md#ground-sources) |
| `roads.json` | The rules per class of road and track, from the most important down: the grade and smoothing of the road solver, the alignment of the bake, the stamp profile of the engine |

## Test

```sh
uv run --locked --group trailview python -m tools.trailview.rules
```

## Pitfalls

- Codes in `materials.json` are indexes in the ground tiles. Append only.
- A region id is a region and a place: one id per region, and one per country or subdivision
  that its pack's `style` or a value by place of its look names. A new region or place changes the ids, so
  the region tiles bake again; the material tiles do not.
- A pack names the engine's models (`builder/web/src/lib/trailview/models/trees/` and `plants/`).
  A new species in a pack needs its model there, or the check fails. A pack holds 32 tree species,
  16 shrubs and 16 ground plants at most, and `styles/` 63 house styles.
