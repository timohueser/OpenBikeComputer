# Parts bridge

`obc-trailview-parts-bridge`: the builder of the tunnel parts,
[`obc-trailview-parts`](../../host/obc-trailview-parts/src/lib.rs), behind `wasm-bindgen`. The viewer's stamp code
(`builder/web/src/lib/trailview/structures/parts.ts`) inlines it as a `data:` URL, in the stamp worker and in the
bake of the stamp layer.

## Build and test

```sh
obc exec bash builder/build-wasm-bridges.sh       # all bridges; this one goes to builder/web/src/lib/trailview/structures/pkg
python3 builder/wasm/wasm_size_guard.py --module parts
obc test -p obc-trailview-parts
obc exec cargo clippy -p obc-trailview-parts -p obc-trailview-parts-bridge --all-targets -- -D warnings
obc exec cargo clippy -p obc-trailview-parts-bridge --target wasm32-unknown-unknown -- -D warnings
```

| Function | Gives |
| --- | --- |
| `meshes(input)` | The count of vertex words, the vertex words (7 per vertex, each word's bits), then the indices |
| `volumes(input)` | The count of box words, the boxes (9 words each), then the floor segments (7 words each) |
| `footprints(input)` | The count of rings, then per ring its count of points and their x, y |
| `built(parts)` | Whether all kinds of a tag `parts` or `ends` are built |

An input is JSON: `{"square": [x0, y0, x1, y1], "ends": [{"at": [x, y], "z": z, "tags": {..}}], "chords": [{"a":
[x, y, z], "b": [x, y, z], "tags": {..}, "code": c, "marked": m}]}`, in world units of the viewer.

## Pitfalls

- The stamp version takes the hash of the tracked sources of the crate, the bridge and `data/trailview/parts.json`
  but their Markdown, not the bytes of the package. The build script writes that hash to `pkg/sources.sha256`.
  `stampBundle` refuses a package whose hash is not the checkout's: build the bridges again after a change of these
  sources. `.gitattributes` keeps their line endings `lf`.
- Two builds of the same sources with another toolchain or other locked dependencies give one version and other
  bytes. A baked node can then draw a part a little differently than the stamp worker.
