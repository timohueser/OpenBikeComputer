# GP4 redesign: probe pictures

Probe code: branch `scratch/gp4-probe` (from `origin/claude/gp3-faces-v3` at 593a5b4), file
`host/obc-ground-plan/src/faces/walk.rs` and small hooks in `faces/mod.rs`, `faces/points.rs`. Throwaway.
All pictures: debug tool, layers `faces` (and `cells`), top-down, north up. Pink: sidewalk faces.
Purple: junction faces. Dark grey: carriageways. Tan: slope band. Blue circles: GP3 rounds.
The probe has no stops (paths and tracks still cut the sidewalk), no drops, no zebras, no buildings,
no run chains across joints, and a fixed 3 m taper.

| Picture | What it shows | Right or wrong |
| --- | --- | --- |
| before/v12-good-town-junctions.png | GP3 head, 128 m | Reference: no sidewalk. |
| before/v11-two-bad-junctions.png | GP3 head, 128 m | Reference. |
| before/zebra-tagged.png | GP3 head, 128 m | Reference. |
| probe/v12-good-town-junctions.png | Probe, 128 m | Mostly right: sidewalks only on walk sides, kerb on the carriageway edge, angled ends at one-walk corners, rings at three two-walk corners. Wrong: a square start at a joint near a junction (see zoom-v12-ring). |
| probe/v11-two-bad-junctions.png | Probe, 128 m | Right: the north sidewalk of the west arm runs straight to the side road and ends angled at the 0.8 m round. |
| probe/zebra-tagged.png | Probe, 128 m | Right: a ring at the two-walk corner (top left), angled ends at one-walk corners. Wrong: a path cuts the south sidewalk (expected: no stops in the probe); a step in the east sidewalk of the road at top left, near its junction (per-way runs, as in zoom-v12-ring). No zebra (not probed). |
| probe/conn-own-a.png | Probe, 128 m | Right: one run along a road and its junction face, angled at both ends, kerb straight. |
| probe/zoom-zebra-ring.png | Probe, 40 m | Rings right. Wrong (corrected after review): at x≈610, y≈160 the east sidewalk tapers into an OSM joint and restarts with a jog, with a shoulder sliver between the two square ends; the 2 m slab also narrows to 1.5 m before each ring foot with a kink. |
| probe/zoom-zebra-junction.png | Probe, 40 m | Right: four one-walk ends, each straight to the side road's edge, angled about 30 degrees, ground behind. Wrong (expected): the path at lower left cuts the sidewalk; GP3's stop rule is missing. |
| probe/zoom-v12-ring.png | Probe, 40 m | Right: the large ring. Wrong: a square start at lower left (x 220): the road is split at a joint 1 m from the junction, and the probe's runs end per way. The design's run chains across joints remove it. |
| probe/zoom-v12-west.png | Probe, 40 m | Right: one both-walk ring (south-east corner) after the run-on rule; three one-walk ends. |
| probe/zoom-v12-north.png | Probe, 40 m | Mixed: a short thin wedge at a junction (a short run between a foot and a bit change). Valid geometry, but it reads as a fleck; see the owner question on short runs. |
| probe/zoom-v11-end.png | Probe, 40 m | Right: straight to the side road, angled end, kerb straight along the junction face. |

Bench (`--faces-bench`, 256 fixture cells, one run each, 4 threads, this shared 4-core machine), GP3 head
593a5b4 against the probe: rural median 7.1 → 7.7 ms, p95 44.2 → 50.0 ms; town median 84.8 → 101.7 ms,
p95 287.9 → 317.5 ms. Check 1 partition faults 0 and check 2 faults 0 on both. Sidewalk artefacts: 3 jogs, 1 sliver.

## probe2 (round 2: owner rules on ends, chains across joints, rounded tapers)

Probe `scratch/gp4-probe` at e44408e (base still 593a5b4). Rules: chains join across open sectors where
both facing sides walk (one stroke, a pie on the outer side); an end against another road (a side road
at a junction, whatever the walk count, unless a two-walk ring) keeps its full width and ends where that
road's centre line crosses the slab's middle line, so the road's carriageway cuts it; a taper only at a
bit change along a road or a free end; end slope 1.0 (45°) and a 1 m fillet at both kinks of the taper.
Still missing: stops (paths cut the sidewalk), drops, zebras, buildings, rounds that ignore stop arms,
the ring radius per foot (finding 6), the cone on the back-edge offset (finding 1).

Cause at the zoom-zebra-ring joint (confirmed with the probe's debug print): ways 170 (north part,
samples run south, ends at sample 14) and 160 (south part, starts at sample 0) meet at an OSM joint
that bends; the round of the joint lies on the west (inner) side. The probe's open-sector test matched
the joint's two-arm sector against that round with the arms reversed, so the east (outer) side lost its
open ends: both east runs got 0 targets (one tapered into the joint, the other out of it), and their
square ends at the joint sample, along the two ways' different normals, left a shoulder wedge between
them on the outer side of the bend. With chains the east side is one chain (ways 160, 170, 169) and one
stroke with a pie at the bend.

| Picture | What it shows | Right or wrong |
| --- | --- | --- |
| probe2/zoom-zebra-ring.png | 40 m | Right: the east sidewalk runs straight through the joint (top right), no jog, no wedge. Wrong: a small step at each ring foot where the 2 m slab narrows to the 1.5 m ring (finding 6: the ring should take each foot's width). |
| probe2/zoom-zebra-junction.png | 40 m | Right: all four ends at the junction run in at full width and the side roads' carriageways cut them straight. Wrong (no stops yet): the path at left cuts the south sidewalk. |
| probe2/zoom-v11-end.png | 40 m | Right: the north sidewalk runs at full width into the side road. Wrong: a small sidewalk fleck at the north-east 0.8 m round (a short run of the east arm's bits, cut by the side road). |
| probe2/zoom-v12-ring.png | 40 m | Right: the ring; the vertical road's slab runs into the junction at the top at full width; the square start at x≈220 is gone. Wrong: a small step at the ring foot at x≈590 (finding 6). |
| probe2/zoom-v12-north.png | 40 m | Right: the former wedge now runs into the road at its bottom at full width and tapers (rounded) at its top. |
| probe2/v12-good-town-junctions.png | 128 m | Right: rings at two-walk corners, full-width ends at side roads, rounded tapers at bit changes. |
| probe2/v11-two-bad-junctions.png | 128 m | Right: one sidewalk, rounded taper at its west end, full width at the side road. Small fleck at the north-east corner (see zoom). |
| probe2/zebra-tagged.png | 128 m | Right: ring top left, ends at junctions at full width. Wrong (no stops): paths cut the south sidewalk. No zebras (not probed). |
| probe2/conn-own-a.png | 128 m | Right: one chain along the road and its junction face, rounded tapers at both bit changes. |
| probe2/taper-27-vs-45.png | 5 taper ends, 24 m each; left 27° (slope 0.5), right 45° (slope 1.0), both with 1 m fillets | Both have round corners at the back edge and run tangentially into the kerb at the tip. 45° is about half as long (2 m slab: 2.4 m against 4.2 m). Row 4 (conn-own-a) shows a short bulge where the end lies close to a junction face. |

Bench round 2 (`--faces-bench`, 256 cells, one run, this machine under load 9 to 11 from other agents,
so absolute times are not comparable): an A/B with the walk layer off and on, alternating, gives medians
that move by 2× between equal runs; the numbers below are only the robust signal:
- check 1 partition faults 0, check 2 faults 0 (off and on);
- sidewalk artefacts: 1 jog, 2 slivers;
- one cell (34296, 23098) slices in about 0.8 to 1.0 s with sidewalks against 31 ms without, every run. The walk stage itself takes 0.8 ms there. Rings, fillets, slope and the one-walk crossing are not the cause (bisected); without chains it is normal. Most likely cause (not confirmed): a sidewalk back edge that runs near and parallel to the rail bed's edge over hundreds of metres in that cell. GP4's implementation must find and fix it; the time gate catches it.

Fixture count (side-road sectors, one line per facing side on a way with `sidewalk_m > 0`, cells overlap so sides near borders count more than once): first segment walks 398, first bit within 2 to 6 m 11, within 12 to 30 m 11, no bit within 30 m 604.
