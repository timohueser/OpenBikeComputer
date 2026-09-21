# Peak View geographic terrain fixtures

Terrain for the three Peak View test sites: Gornergrat, Kleine Scheidegg and
Kaiser-Franz-Josefs-Höhe. The simulator computes the panorama at runtime with the allocation-free
renderer in `firmware/obc-app/src/peak_view/surface.rs` and the production OBCT surface reader.

Observer locations and the peak catalogue are in [locations.json](locations.json). Fixture summit
coordinates are reconstructed from rounded bearings and distances; production map summit records
keep exact OSM coordinates.

## Obtain

`obc fixtures sync sim-peak-view`. The package is also in the `sim` profile, and the simulator
resolves it through the standard fixture cache. `OBC_PEAK_TERRAIN_DIR` overrides it for a local
bake. **The simulator never downloads data.**

## Generate

Install NumPy and Pillow, then run from the repository root:

```sh
cargo build --release -p obc-dem
python3 fixtures/generate_peak_view.py --download
cargo fmt --all
```

Without `--download`, generation uses only cached Terrarium tiles and fails if one is missing;
`--cache PATH` selects that input cache. The output directory defaults to
`~/.cache/openbikecomputer/peak-view`, and `--terrain-dir PATH` changes it. `--dem-tool PATH`
selects another build of the production converter. **A local bake does not replace the registered
package.**

The generator writes `gornergrat.obcd`, `scheidegg.obcd` and `glockner.obcd` as OBCT v3, plus the
Rust catalogue in `catalog.rs`. Each file holds native heights, a full height pyramid,
conservative approximation errors, and maximum-height bounds within and across geographic cells.
No panorama, visibility result or lighting layer is stored, so any observer inside the coverage
can use the same file.

The source grids use postings of 2^9, 2^10 and 2^11 microdegrees, source zooms 12, 11 and 11, and
coverage radii of 28, 53 and 103 km. The generator puts the finest available source at each
coordinate onto one 2^9-microdegree lattice; coarser outer sources extend coverage and
interpolation adds no detail. `obc-dem surface` then creates the lower levels and the bounds.
Geographic cells span 2^16 microdegrees. Runtime level changes happen at 25 and 50 km and all rays
end at 100 km, so the extra coverage is what lets a nearby observer position work.

`--source-levels PATH` reuses the three source grids from an earlier bake. That directory must
hold `{site}-9.obcd`, `{site}-10.obcd` and `{site}-11.obcd` per site, and this path reads and
downloads no Terrarium tiles.

Large terrain files stay in the fixture cache. The published package also carries this README, the
generator, and the location and Rust catalogues.

## What the panorama is and is not

Lighting is fixed relative to geography, 25 degrees above the horizon and to the north-west. It
describes terrain form only: **no snow, no rock texture, no cast shadows and no real sunlight.**
Source resolution and level changes limit detail. It is not a photographic reconstruction.

Named summits use measured terrain at their catalogue distance and within half a degree of their
bearing, with a quarter-degree tolerance on visibility. **Terrain is never raised to meet a
catalogue height.** Moving the observer recomputes the relative bearings and distances.

The completed 960 × 222 image is two bits per pixel. It, the builder's working buffers, the
terrain reader and the caches fit inside the existing 128 KiB arena; app metadata, host thread
state, file objects and the display framebuffer are outside that accounting. Turning reads the
completed image and touches no terrain storage.

Terrain starts 2 m from the observer. Curvature uses an effective Earth radius with refraction
coefficient 0.13.

A GPS displacement above 20 m starts a replacement job; smaller changes keep the current panorama,
to limit GPS jitter. At a moved observer, ground elevation plus two metres is the viewing height.
Missing distant terrain and clipped coverage show a partial-coverage notice and bearing marks.
Missing observer terrain or a failed read is an unavailable state.

Headless scripts send `f` to wait for real generation before Browse gestures; there is no
artificial delay. The logs report host generation time, source reads, bytes and working-set size,
and the final frame timing excludes generation. **Host timings do not establish device timings.**

## Source and attribution

Elevation: [Mapzen terrain tiles on AWS](https://registry.opendata.aws/terrain-tiles/), Terrarium
PNG encoding. The provider does not version these tiles, so a fresh download can change generated
heights.

Attribution: [Mapzen and its elevation data providers](https://github.com/tilezen/joerd/blob/master/docs/attribution.md).
Europe terrain data produced using Copernicus data and information funded by the European Union —
EU-DEM layers. Austria terrain data © offene Daten Österreichs — Digitales Geländemodell (DGM)
Österreich. Global terrain includes USGS SRTM data.

Peak catalogue: © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright).
