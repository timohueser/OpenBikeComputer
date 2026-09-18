# Shade-selection variants

What was compared to choose the terrain settings in `bake.py`. Every variant used the same rock
clip, the same 25 m grid, the same pattern F, and the same views. Only the lighting and the
threshold changed. "Ink" is the share of the rock the variant shades.

| Variant | Azimuth | Altitude | Smoothing | Threshold | Ink | Shade polygons |
| --- | --- | --- | --- | --- | --- | --- |
| V0 control | 45 (NE) | 45 | 30 m | 0.56 | 41.9% | 1,033 |
| V1 light only | 315 | 45 | 30 m | same ink | 41.9% | 1,105 |
| V2 generalised | 315 | 45 | 120 m | same ink | 41.9% | 827 |
| V3 less ink | 315 | 45 | 120 m | 30th percentile | 30.0% | 718 |
| V4 high sun | 315 | 60 | 120 m | 30th percentile | 30.0% | 740 |
| **V5 chosen** | **315** | **45** | **200 m** | **flat − 0.10** | **43.1%** | **755** |
| V6 pure aspect | 315 | 45 | 200 m | flat | 57.6% | 866 |
| V7 two levels | 315 | 45 | 200 m | V5 plus deep 18% | 43.1% | 1,119 |

## What each step showed

**V1 — the light direction alone.** With the ink held at the control's 41.9%, moving the light
from northeast to northwest changes which flanks are shaded, not how much. On the test massif the
control shades almost nothing, because the northeast light lit the flanks that face away from a
reader's expectation. This was a sign error, not a tuning choice.

**V2 — generalisation.** Raising the Gaussian from 30 m to 120 m drops the median rock slope from
34 to 30 degrees and replaces speckle with regions. The shade polygon count falls from 1,105 to
827 at identical ink, so generalisation is cheaper, not more expensive.

**V3, V4 — less ink, higher sun.** Both are legible. Neither changes where the boundary falls, so
neither fixes the "the edge looks arbitrary" complaint.

**V5, V6 — anchoring the threshold.** At 45 degrees altitude flat ground has illumination 0.707.
A threshold at that value tests aspect alone, so the shaded edge lands exactly where the aspect
flips: the ridge and valley lines. V6 does that and shades 57.6%. V5 sits 0.10 below, which leaves
gentle ground unshaded and keeps the edge close to the ridge at 43.1% — the control's ink, placed
where it means something. V5 was chosen for that reason.

**V7 — a second level.** Measured, off by default. See the README.

## The hillshade check

`illumination is below a threshold` is a claim about landform, so it was checked against a
continuous hillshade of the same window rather than by eye alone. For the test massif at
16 m/pixel:

- the continuous northwest hillshade at 200 m smoothing shows a branching valley system;
- the control's selection is scattered speckle that does not follow it;
- V5 and V6 reproduce the same branching bands.

This is the evidence that the new selection carries landform information and the old one did not.
It is a host-side comparison of the bake input, not a device measurement.

## What was not changed

The shading is clipped to OSM `bare_rock`, `scree`, and `shingle`, with water and glaciers removed.
A massif's form therefore breaks at the vegetation line rather than at a ridge. This is deliberate:
the feature is about high mountain terrain, and shading everything costs too much. The clip is
settled, not an open question.

## Second round: does the texture carry useful information?

The light-based shading above reads as landform but does not tell a rider anything they act on.
This round asked whether a different criterion, or a stronger tone, changes that.

| Variant | Criterion | Ink | Shade polygons |
| --- | --- | --- | --- |
| A | light, `--aspect-drop 0.10` | 43.1% | 755 |
| B | steepness, `--slope-above 35 --smooth-m 200` | 20.0% | 481 |
| C | hollows, `--hollow-radius 250 --smooth-m 80` | 28.0% | 977 |

**Dot density turned out to be the larger lever, and it is free.** The style table's fill colour
selects the density; the polygons, the file, and the render counters do not change. At 6.25% the
shading is hard to see at all, at 12.3% it reads at a glance, and at 21.9% it approaches a solid
tone and competes with the trails, which is what the earlier solid dark-grey fill did.

**One criterion failed for a structural reason.** Hollows measured over a 150 m radius pick out the
gully and drainage network cleanly as a signal. The bands are 5 to 10 pixels wide, and a dot
texture needs about 15 to 20 pixels to read as a tone, so almost none of it renders. Nothing about
the threshold fixes that: this pattern can only express regions wider than roughly 250 to 350 m on
the ground at 16 m/pixel. Narrow terrain features would have to be lines, which is a different
visual language.

## Outcome

Parked on 2026-09-18. With all three criteria legible at 12.3%, none of them was judged to add
enough to the map to pay for the polygons, the extra bake step, and the reserved colours. The
branch is kept as a record.

The one result that did carry its weight was the contour colour, which is independent of the
shading and was taken forward on its own.
