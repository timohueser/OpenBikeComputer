# Peak View geographic terrain fixtures

The simulator computes panoramas at runtime for Gornergrat, Kleine Scheidegg and
Kaiser-Franz-Josefs-Höhe. It uses the allocation-free renderer in
`firmware/obc-app/src/peak_view/surface.rs` and the production OBCT surface reader.
The observer locations and peak catalogue are in [locations.json](locations.json).
Peak names, heights, distances and bearings come from the original 2026-08-30 fixtures.
Those inputs retained rounded bearings and distances, so fixture summit coordinates are
reconstructed from them. Production map summit records retain exact OSM coordinates.

## Obtain

Run `obc fixtures sync sim-peak-view`. The package is also part of the `sim`
profile. The simulator resolves the package through the standard fixture cache.
`OBC_PEAK_TERRAIN_DIR` overrides it for a local bake.

## Generate

Install NumPy and Pillow, then run from the repository root:

```sh
cargo build --release -p obc-dem
python3 fixtures/generate_peak_view.py --download
cargo fmt --all
```

Without `--download`, generation uses only cached Terrarium tiles and fails if one is
missing. `--cache PATH` selects that input cache. The output directory defaults to
`~/.cache/openbikecomputer/peak-view`; `--terrain-dir PATH` changes it. Set
`OBC_PEAK_TERRAIN_DIR=PATH` when running the simulator with a different directory.
The simulator does not download data. A local bake does not replace the registered
package. `--dem-tool PATH` selects a different build of the production converter.

The generator writes three OBCT v3 files: `gornergrat.obcd`, `scheidegg.obcd` and
`glockner.obcd`. It also writes the Rust catalogue in
`fixtures/sources/peak-view/catalog.rs`. Each geographic file contains native heights,
a full height pyramid, conservative approximation errors, and maximum-height bounds within and across
geographic cells. No panorama,
visibility result, or lighting layer is stored. Any observer within the coverage can
use the same file.

The source grids use postings of 2^9, 2^10 and 2^11 microdegrees, source zooms 12, 11
and 11, and coverage radii of 28, 53 and 103 km. The generator puts the finest available
source at each geographic coordinate onto one 2^9-microdegree lattice. Coarser outer
sources extend coverage; interpolation does not add detail. The production
`obc-dem surface` converter then creates the lower-resolution levels and bounds.
Geographic cells span 2^16 microdegrees. Runtime level changes occur at 25 and 50 km;
all rays end at 100 km. The extra coverage permits nearby observer positions.

To reuse the three geographic source grids from an earlier bake, add
`--source-levels PATH`. That directory must contain `{site}-9.obcd`, `{site}-10.obcd`
and `{site}-11.obcd` for each site. This path does not read or download Terrarium tiles.

Large terrain files stay in the fixture cache. The published package also contains
this README, the generator, and the location and Rust catalogues. The same OBCT surface
layout is available to map assembly; these files select the data for the three test
areas.

## Rendering and interaction

The host starts one cancellable worker when Peak View opens. Each sector combines 15
output rays with neighboring rays for outlines and quarter-degree rays for summit visibility.
It checks baked height bounds from near to far and skips blocks that cannot reach the
next visible pixel. Within the remaining cells, it computes intersections with the
bilinear terrain surface. Each step has a bounded hierarchy-node budget. The current
heading gets priority. The view appears when its terrain and nearby catalogue rays
are complete; the remaining sectors continue in the background.

The completed 960 by 222 image uses two bits per pixel. The image, builder working
buffers, terrain reader and caches fit within the existing 128 KiB memory arena.
App metadata, host thread state, file objects and the display framebuffer are outside
this accounting. Turning reads the completed image and does not access terrain storage.

Only visible surfaces receive shading. Lighting and depth are computed at each visible
native cell span's endpoints and interpolated between them. Large smooth patches use exact
per-row intersections. Their slopes determine face lighting, and
depth discontinuities determine outlines. Terrain starts 2 m from the observer.
Coordinates use a local geographic projection. Curvature uses
an effective Earth radius with refraction coefficient 0.13.

Lighting is fixed relative to geography, at 25 degrees above the horizon and to the
northwest world direction. It describes terrain form, not snow, rock
texture, cast shadows, or actual sunlight. Source resolution and level changes can
limit detail. The panorama is not a photographic reconstruction.

Named summits use measured terrain at their catalogue distance and within half a
degree of their bearing. Visibility uses the intervening terrain with a quarter-degree
tolerance. Terrain is never raised to meet a catalogue height. Moving the observer
recomputes the relative summit bearings and distances.

The shared compass spinner appears until the current view is ready. Turning toward
an unfinished view gives it priority. Back cancels; opening again starts a new job. A GPS displacement above 20 m starts a replacement job. Smaller changes keep
the current panorama to limit GPS jitter. At a moved observer, ground elevation plus
two metres sets the viewing height. Missing distant terrain and clipped coverage show a partial-coverage notice and bearing marks.
Missing observer terrain or failed reads cause an unavailable state.

The GUI opens the selected preset directly. Headless scripts use `f` to wait for
actual generation before sending Browse gestures. There is no artificial loading
delay. Logs report host generation time, source reads, bytes and working-set size;
final frame timing excludes generation. Host timings do not establish device timings.

## Source and attribution

Elevation: [Mapzen terrain tiles on AWS](https://registry.opendata.aws/terrain-tiles/),
Terrarium PNG encoding. The local source cache was read on 2026-09-12. The provider
does not version these tiles; a fresh download can change generated heights.

Attribution: [Mapzen and its elevation data providers](https://github.com/tilezen/joerd/blob/master/docs/attribution.md).
Europe terrain data produced using Copernicus data and information funded by the
European Union — EU-DEM layers. Austria terrain data © offene Daten Österreichs —
Digitales Geländemodell (DGM) Österreich. Global terrain includes USGS SRTM data.
Peak catalogue: © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright).
