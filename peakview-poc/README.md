# PeakView proof of concept

This throwaway Python renderer tests a PeakFinder-style screen at the OpenBikeComputer's native
240 x 268 resolution and 64-colour RGB222 palette. It is deliberately self-contained and does not
share code or formats with the device, with one exception: text is drawn from the firmware's own
Terminus 12 x 24 glyph strip (`firmware/obc-render/fonts/terminus/ter_u24b.raw`, the Label tier),
so every name renders the exact pixels — and the exact space cost — the device would. The display
uses a white background, progressively darker green terrain layers, amber for the heading marker,
the selected peak, and the sun, and the device's route magenta for the route crossing.

## Setup and use

Python 3.10 or newer is required.

```sh
cd peakview-poc
python3 -m venv .venv
. .venv/bin/activate
pip install -r requirements.txt
python peakview.py
```

The default is Gornergrat at 45.9834 N, 7.7854 E, looking towards 220 degrees. It writes `out.png`
at native resolution and `out_4x.png` as a nearest-neighbour monitor preview.

```sh
python peakview.py --preset kleine-scheidegg --out examples/kleine-scheidegg.png
python peakview.py --lat 45.9834 --lon 7.7854 --heading 220 --fov 120 --eye-height 2
python peakview.py --preset grossglockner --strip --out grossglockner.png
python peakview.py --preset gornergrat --peak-step 1 --out gornergrat-next-peak.png
```

Available presets are `gornergrat`, `kleine-scheidegg`, `grossglockner`, and `feldberg`. Useful
experimentation flags include `--max-range` (kilometres), `--max-labels`, `--min-score`,
`--visibility-tolerance`, `--scale`, and `--wikidata-prominence`. Three clean green terrain layers
are drawn by default; use `--layers 4` for another depth step or `--layers 1` for the original flat
silhouette. `--strip` additionally produces a 720 x 268 full-360-degree image. Run `python
peakview.py --help` for the complete CLI.

The quiet default draws at most five major peak names, each as a vertical label rising from its
summit (a horizontal name in the device font would span most of the screen). A label slides down
over the terrain when the sky above its peak is too short; a one-pixel white halo keeps it readable
there. One peak is selected independently and marked with an amber triangle, echoed in the bottom
panel beside its full name, elevation, distance, and compass direction. Selection starts at the
peak nearest the heading line. The intended device controls are Up for the previous peak and
Down for the next peak in azimuth order, wrapping at both ends. Since this renderer produces static
PNGs, `--peak-step -1` and `--peak-step 1` preview those button states.

Two overlays preview how PeakView would tie into the rest of the product:

- **Sun.** A real solar ephemeris (NOAA approximation) with a mocked clock: `--date`, `--time`
  (default 17:30), and `--tz` (default CEST). When the sun is low enough to be in frame it is drawn
  as an amber disc; independently, the renderer scans forward in one-minute steps for the next
  moment the sun crosses the *computed skyline* — sinking behind a ridge or clearing one — and
  marks that spot with a half-sunk amber disc and the clock time. In the mountains this differs
  from geometric sunset by hours, which is exactly the camp-spot question. From Kleine Scheidegg
  it finds the morning sun emerging past the Eiger's shoulder. `--no-sun` disables it.
- **Route.** A mock route (straight ride along `--route-bearing`, default heading + 8, capped at
  25 km) sampled against the DEM to find where it disappears over the skyline — the pass you are
  heading for. Marked with two climbing chevrons in the device's route magenta plus the distance.
  On device this would walk the active route's real polyline. `--no-route` disables it.

Overlays claim screen space in glanceability order — selected peak, sun, route — and the generic
name labels fill the space that remains, so a label is dropped rather than colliding.

Rendered native and 4x examples for every preset are in `examples/`; Gornergrat also includes the
360-degree strip.

Downloaded Terrain Tiles, Overpass responses, and optional Wikidata responses live under
`cache/`. Once a location and range have been rendered, `--offline` verifies that it can be rendered
without network access. A different heading can reuse most DEM tiles, but may need tiles not touched
by the first view.

## How it works

Elevation comes from the public AWS Terrarium tile set at zoom 11. The renderer ray-marches every
screen azimuth with 30 metre steps nearby, increasing to 200 metres at long range. At each step it
computes the apparent terrain angle after Earth-curvature and standard-refraction correction; the
largest angle is the skyline. Distance-band edges come from clusters of visible ridge-crest
distances in the current view, rather than fixed kilometre ranges. The bands are drawn far-to-near
so foreground ridges occlude distant ones. Named peaks come from OpenStreetMap via Overpass. A peak
is visible when its corrected apparent angle reaches the combined skyline within a small tolerance.
Labels are ranked by an OSM prominence tag when present, otherwise elevation times estimated
isolation, and greedily placed without overlap. Optional Wikidata P2660 lookups can replace that
estimate.

The final image is explicitly reduced to the device's channel values 0, 85, 170, and 255. Text is
blitted 1:1 from the firmware glyph strip (rotated 90 degrees for the skyline labels), geometry is
drawn without resampling, and the enlarged preview uses nearest-neighbour scaling.

## Known limitations

- The zoom-11 DEM is roughly 50 metres per pixel in the Alps, so small spires and exact summit
  heights are softened.
- OpenStreetMap peak names and elevations are community data and can be incomplete.
- Label placement is intentionally simple; dense panoramas favour isolated, high summits.
- Trees, buildings, local geoid differences, and shaded mountain faces are not modelled.
- The default 120-degree Gornergrat view spans bearings 160 to 280 degrees. Dufourspitze/Monte Rosa
  is at about 129 degrees and Dent Blanche at about 293 degrees, so neither can truthfully appear in
  that crop. The full strip includes the Monte Rosa direction.
