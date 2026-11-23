# TrailView model files

TrailView draws each model of one fixed shape (a tree species in a season phase, a plant, a rock, an animal,
a bench or another small object) many times. The web build makes these models and writes them as model
files beside the bundle. The engine fetches a file and writes its bytes into the instance pools as they are.
This contract defines the bytes of a file, the manifest that names the files, and the budget that each
model keeps, whoever makes it.

- Version: file `version` `1`. A reader refuses a file of another version or with another magic number.
- Writers: `builder/web/src/lib/trailview/models/bake.ts`, which `builder/web/vite/model-files.ts` runs
  in the build.
- Readers: `decodeModels` in `builder/web/src/lib/trailview/models/model-file.ts`.
- Code authority: `models/model-file.ts` (the layout and `quantize`), `models/pack.ts` (the values that a
  file holds of each model), `models/bake.ts` (the files and the budget).

## Conventions

- All numbers are little-endian. `u32` is an unsigned 32-bit integer, `f32` an IEEE 754 single.
- A model is 1 tall in model units, with its base at the origin and z up.
- "Coded" means the meshopt codec of the `meshoptimizer` package: `encodeVertexBuffer` with a stride
  of 16 bytes, and `encodeIndexSequence` with 4-byte indices, which keeps the order of the indices.

## 1. Files and the manifest

The build emits each file as an asset with the hash of its content in its name, so a changed model
has a new address. The module `virtual:models` of the build is the manifest (`Models` in
`model-file.ts`): the address of each file, and what the engine needs before a file arrives.

| File | Models, in order | Manifest entry |
| --- | --- | --- |
| A tree species' summer crown | 1, with 3 tiers | `trees[id].summer`, with `keepsHeading` and, per phase (winter, spring, summer, autumn), the colour matrix (3 × 3, columns first) that turns the summer crown into the phase's, or null |
| A tree species' crown of its own in a phase | 1, with 3 tiers | `trees[id].crowns[phase]`, null for a phase without one |
| A plant in a season | 3 shapes (seeds 1, 2, 3), 1 tier each | `plants[id].seasons[season]`, with the plant's `ground`; a season in which the plant has no model has no file and no key |
| A breed of animal | 1 per coat and pose, coat by coat (model `coat × poses + pose`), 2 tiers each | `breeds[name].file`, with `height`, `coats` and `poses` |
| The objects | the rocks (3 slabs, 3 boulders), then the models of the kinds in the order of `POINT_MODELS` | `objects.file`, with `rocks` and per kind model the end of each tier in metres (`until`) and `casts` |

## 2. File layout

| Field | Type | Value |
| --- | --- | --- |
| magic | `u32` | `0x464d5654` (`TVMF`) |
| version | `u32` | `1` |
| models | `u32` | the count of models |
| heads | | one head per model, below |
| blobs | | per model, per tier and then for its proxy: the coded vertices, then the coded indices |

A head:

| Field | Type | Value |
| --- | --- | --- |
| extent | `f32` | the largest absolute coordinate of all its tiers |
| radius | `f32` | the radius of a sphere about (0, 0, 0.5) that holds all its tiers |
| spread | `f32` | the farthest that a vertex lies from the z axis |
| crown | `f32` | a tree's lowest crown point, NaN for a model without a crown |
| sinks | 8 × `f32` | a tree's sink table (`slope_sink` of `emit.wgsl`), zeros without a crown |
| flags | `u32` | bit 0: a tier has a trunk; bit 1: the model has a shadow proxy |
| tiers | `u32` | the count of mesh tiers, finest first, 3 at most |
| counts | 2 × `u32` per tier, and for the proxy | the vertex count and the index count |
| outlines | 256 × `f32` | per impostor frame (8 × 8) the trapezoid that holds the last tier: bottom, top, half widths |

A blob is a `u32` byte length, then the bytes, padded with zeros to a multiple of 4. So a file is a
multiple of 4 bytes long.

## 3. Vertices

A vertex is 16 bytes, as the instance pipeline reads it:

| Bytes | Type | Value |
| --- | --- | --- |
| 0–5 | 3 × snorm16 | the position over `extent` |
| 6–7 | int16 | for a vertex of the trunk, the trunk's top in model units × 32767; else 0 |
| 8–10 | 3 × snorm8 | the normal |
| 11 | int8 | 127 for a vertex that rises at dusk, else 0 |
| 12–14 | 3 × unorm8 | the colour in sRGB |
| 15 | unorm8 | 255 − 255 × its glow at night |

Indices are three per triangle. The proxy is a mesh of 6 vertices in this format, packed over the extent as the head holds it (rounded to float32).

## 4. Model budget

Each model keeps the budget of its kind, and the build fails on a model that does not. A model that
appears in small numbers on screen may have several times the triangles of a tree's far tiers; trees and plants,
which fill the screen, keep their counts. `BUDGET` in `models/bake.ts` holds the numbers and says
where each comes from.

| Model | Tier | Triangles, at most | Vertices, at most |
| --- | --- | --- | --- |
| Tree | 0 | 4,700 | 9,000 |
| Tree | 0, the mean of the summer crowns of all species | 3,400 | |
| Tree | 1 | 900 | 1,800 |
| Tree | 2 | 230 | 600 |
| Plant, each shape | 0 | 350 | 1,000 |
| Rock | 0 | 100 | 300 |
| Animal | 0 | 1,000 (the owner's choice: few on screen) | 3,000 |
| Animal | 1 | 250 | 750 |
| Small object of a kind | 0 | 600 (the owner's choice: few on screen) | 1,800 |

One view draws at most 256 instance models, each with at most 3 mesh tiers and an impostor. The
same files serve the desktop and the phone; a phone draws nearer tier ends through the budget table
of the engine.
