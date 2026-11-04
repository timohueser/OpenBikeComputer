Status: approved
Redesigned on GP3's approach after the owner's rules; reviewed (xhigh, two rounds); owner decisions in 'Owner decisions'.
Part of #3096. One pull request into ground-plan.
Depends on: #3112 (GP3 at `origin/claude/gp3-faces-v3` bc4f0a8 or later, with the hooks in "Interfaces"), GP18 (the `service` tag on roads, merged)

## TL;DR

**Goal:** Each side of a way that the bake's `walk` marks has a sidewalk in the ground plan. The kerb is the carriageway's edge and stays straight. Where both arms of a corner have a sidewalk, it follows the kerb round the corner. Where a sidewalk meets another road, it keeps its full width and runs in; that road's carriageway cuts it. Only where it runs against nothing (a `walk` change along a road, a free end) does it angle toward the road, with round corners. Driveways, tracks and paths stop at the sidewalk's back edge, and the host's sidewalk runs on. The kerb lowers at driveways, tracks, paths that run on across, and zebras. Zebras come from their OSM tag. Slabs and shoulders give way to buildings. Every result is a valid plan in every cell.

**Problem:** GP3 gives carriageways, junction faces, shoulders, verges and slopes, but no sidewalk. The first GP4 design built sidewalks from special pieces (an obstacle-end solver with retries, a way frame, a `KerbProfile` with stations, a minimum-width end, a pinch exception). That style failed twice in GP3. GP3 now builds every face from a few general operations: integer strokes, rounds per pair of neighbouring arms, point regions with straight seams, one integer slice, and a label per patch by the least key.

**Decision:** A sidewalk is one more band layer: a one-sided stroke of a **chain** out to a back-edge offset `b(s)`. GP4 decides `b(s)` and the kerb's rise along each chain; GP3 strokes that side's other bands from the same points, and a few small rules in GP3's one slice do the rest.
- **Chains.** The walking stretches of the way sides join into chains through every graph point where both facing sides walk across a sector of half a turn or more (one stroke, a pie on the outer side). A chain splits only at a round with a foot on both arms (a two-foot round).
- **Ends.** A chain end that meets another road keeps its full width and runs on to where that road's centre line crosses the slab's middle line; the road's carriageway, which ranks over the sidewalk, cuts it. A chain end against nothing (a `walk` change along one road, a free end) has the width 0.
- **One width rule.** `b(s) = min over all constraints (h_i + w_i + k · |s − s_i|)`, then a fillet of radius `end_round_m` at each kink. `k = end_slope`. The constraints are each sample's carriageway edge plus class width, and each 0-target end. The kerb never moves; only the back edge angles in, with round corners.
- **Corners.** At a junction, the count of facing sides that walk picks the corner radius from the table. At a two-foot round where both walk, a ring carries the sidewalk round the corner; its inner radius runs from `ρ − w_a` to `ρ − w_b`, and it overlaps both chain ends.
- **The same slice decides the rest.** Ground buildings are masks of ranks. A stop guest's own candidates do not count inside its stop mask (the host's stroke out to the stop layer), so it ends on the host's back edge in its own surface. A zebra is a post-label surface rule, like GP3's rail crossing.
- **Heights for GP6.** Per kerb edge, marks along it with the slab's rise `R` and the step height `F` (`F < R` where a drop lowers the kerb), linear between marks. The slice gets no extra vertex for them.

**Rejected:**
- The first design's special pieces: each needed its own repair; the slice already decides every overlap.
- A taper at every chain end (round 1): the owner rejects it where the sidewalk meets a road. It runs in, and the overlay cuts it.
- Chains that stop at every way end (round 1): a square start and a shoulder wedge at an OSM joint (`probe/zoom-zebra-ring.png` against `probe2/zoom-zebra-ring.png`).
- Joining chains through a sector under half a turn: at an acute angle the slab passes the other arm's far kerb (review finding 5).
- A taper that shortens to fit a short chain: a spike of full width (probe, first run). The cone never shortens.
- Exact shared vertices between a ring and the chain ends: the end widths must then equal the ring width, which forces narrowings and steps (`probe2/zoom-zebra-ring.png`). The ring takes each foot's width and overlaps the chain end instead.
- Sample insertion into GP3's lines for GP4's breakpoints: it breaks GP3's index-based state and rounds edge points again (finding 10). GP4's strokes carry their own interpolated points; the band strokes of a chain's side take the same points.
- Cut lines across the slab to give the kerb vertices at breakpoints: a cut that ends inside a face leaves a slit (delta finding 2). The breakpoints travel as marks on the kerb edges instead.
- One label for all sidewalks: unrelated slabs (hairpin legs, parallel roads) merge into one face, and GP6's nearest kerb jumps inside it (finding 9a). A sidewalk patch takes the way of its nearest chain as its material.
- A taper where a building cuts the slab (owner decision): it needs the obstacle-end solver. The slab ends on the facade.
- A run-on of the bits to a corner (`corner_look_m`, round 1): on the fixture, 398 facing sides at side-road sectors walk on their first segment and 11 start 2 to 6 m out (counted by the probe). The cause is not the bake's bed rule (it skips 12 m about a shared node). Owner decision 7: the bake fixes those 11 (a follow-up in GP18's area); GP4 builds no run-on.

**Systems:** trailview
**Contracts:** none (the face plan is internal to the crate)
**Size:** L (GP4's own code about 900 lines: `faces/walk.rs` about 450, `faces/kerbs.rs` about 200, `faces/zebras.rs` about 100, tests about 250; GP3 hooks about 350 lines, most of them H1 and H6)

## Facts

- `walk`: 1 is the left of the part's direction, 2 the right; a reader ignores it at a part's last vertex (`specs/trailview-features.md:283`, `:305`). GP3's stroke normal `left(u) = (−u_y, u_x)` (`faces/stroke.rs`) is the spec's right, so stroke side +1 is bit 2. `Sample::walk` is the bit of the span from the sample on (`graph/curve.rs`, `sample`).
- The bake's bed rule skips a way within 12 m of a point where it meets the road (`JOIN_M`, `host/obc-trailview-features/src/roads/sidewalks.rs:33`, `clear` at `:283`), a rail at a shared-node level crossing included. Fixture count (probe): at side-road sectors, of the facing sides on roads with sidewalks, 398 walk on their first segment, 11 start 2 to 6 m out, 11 start 12 to 30 m out, 604 have no bit within 30 m (sides near cell borders count once per cell).
- Widths: 2 m for primary to tertiary and their links, 1.5 m for unclassified and residential, 0 for the rest; `kerb_m` 0.12 where `sidewalk_m > 0`; `verge_m` 0 (`data/trailview/roads.json`).
- GP3 at bc4f0a8:
  - `plan.json` has no `straight_deg` and no `turn_deg`.
  - A joint has pairs on both sides (`faces/mod.rs` `graph_points`, `pairs: vec[(0, 1, radius), (1, 0, radius)]`, radius `inner_radius_m`).
  - A round is either two-foot (`Fit::Edges`: tangent to both facing edges, under half a turn) or one-foot (`Fit::Leaves`: tangent to one arm's edge where it leaves another arm's square end or a gap fill; no band rings) (`faces/points.rs`, `round`, `leave`, `leave_fan`).
  - The band layers are strokes from the centre line at offsets that are the same on both sides. Each layer is one candidate with one label (`Owner::Terrain`).
  - A round's band ring has its arc at `ρ − wide` and is skipped where `ρ − wide < INSET`.
  - The label rule takes the least key, then a cut's `beyond` label, then the junction region's relabel (`faces/mod.rs`, `label`, about `:940`), then the rail-crossing rule.
  - A junction corner takes the radius of its lesser class: `corner_radius_m`, or `corner_radius_walk_m` where either facing side walks (`roads.json` class columns; `faces/mod.rs` `sidewalk`, about `:310` to `:350`). `pairs` carry no radius. The stop mouth is not built.
  - A seam that ends inside a face would leave a slit; GP3 trims each seam to its carriageway's ring and inserts the crossing vertices (`seams_on_carriageways`, about `:788`).
- Kinds: the road kind keeps `service`; the path kind keeps `footway`, `crossing`, `crossing:markings`; the crossing point kind keeps `crossing`, `crossing:markings`; the building kind keeps `building`, `building:min_level`, `min_height` (`data/trailview/kinds.json`).
- Today's viewer: lowered kerb 2.5 cm, ramps 1.2 m, half-width plus 0.5 m over the sine with a floor of 0.3; a path lowers the kerb where it runs on across (cosine −0.5) or where `footway=crossing`; zebra 2 m each way, reach 2.5 m (`roads/marks.ts`, `roads/junctions.ts`).

## Words

- **Side:** +1 or −1 in GP3's stroke frame; `walk` bit 2 or 1.
- **Station, run metres:** GP2's `(span, s_m)`; metres between two stations from span lengths and `s_m`. GP4 never reads `along_m`.
- **Sector, facing side:** the space between two neighbouring arms of a graph point, in heading order; arm `a` turns toward `b` through it. Its facing sides are `a`'s left and `b`'s right. On the way, the side is the arm side times the arm's `dir`.
- **Stop arm:** an arm whose pair with its host has the mouth `stop` (paths, tracks, steps, driveways). GP4's sectors and GP3's corner rounds skip stop arms (H6).
- **Same road:** two arms of one joint, of one way's turn, or the two host arms of a junction. Any other pair of arms meets "another road".
- **Two-foot round:** GP3's round tangent to both facing edges: centre `c`, radius `ρ`, a foot on each arm. A one-foot round is GP3's "leave" round.
- **Walks at a sector:** the facing side's bit is set on the arm's first segment.
- **Stretch:** a maximal run of segments of one way side with the bit set.
- **Chain:** stretches joined through open joins, cut at two-foot rounds, ended by the end rules. Each chain is a list of `(way, side, station range)` pieces in order.
- **P (run-in point):** where the other arm's centre line crosses the chain's middle line (offset `h + w/2`), on the chain's own road (its line, then its through partner arm at the point).
- **Constraint:** `(s_i, b_i)`: a sample's `h + w_class`, or a 0-target end's `h`.
- **End slope `k`:** metres of width per metre of chain at a taper.
- **Rise share `r(s)`:** `w(s) / w_full(s)`, with `w_full` the width under the same rule with no 0-target end.
- **Group:** the chains connected through a join, a ring, or a sector where both facing chains run into each other. Two sidewalk faces of one group are related; of two groups, unrelated.
- **Kerb line:** a chain's carriageway edge on its side, and every round's arc that a chain's slab meets (two-foot and one-foot).
- **Ground building:** an area of the kind `building` whose `building` is not in `walk.over_buildings`, with no `building:min_level` or `min_height` above 0.

## Data that GP4 reads

- GP2: `Sample::walk`, stations, span lengths, graph points with arms, hosts and `dir`, the way's level, the part's feature for its tags.
- GP3: `WayLine` samples and half-widths, the pair rules, per graph point its sectors with their rounds (two-foot: centre, radius, feet; one-foot: circle), the regions, and the slice.
- Ground buildings whose box meets a chain's or a shoulder's box (outer rings; holes are not read).
- Crossing points with `crossing` and `crossing:markings`; the path tag `footway`.

## Data that GP4 writes

- **To GP3, per side with a chain:** the chain's point list (GP3's samples of that stretch plus GP4's interpolated points, each with its station and segment direction) and per point the back edge `b` and the shoulder share `q`. GP3 strokes the band layers of that side from this list (H1), so the shoulder's and the slope's inner edges are the same points as the slab's back edge: no strip that depends on the sample spacing, no near-coincident edges. GP3's layer offsets on that side: sidewalk `b`; paved shoulder `b + paved · q`; shoulder `b + shoulder_m · q`; verge `+ verge_m`; slope `+ reach`. The carriageway keeps GP3's own samples.
  - On a chain, `q = 0`: no shoulder behind the slab. So ground (verge and slope) fills behind a taper, and no paved wedge shows (#3040).
  - Off a chain, `q = min(1, k · d / shoulder_m)`, with `d` the run metres to the nearest chain end of that side. So past an end the shoulder starts at the end slope, and it does not start square.
  - A side with no chain within `shoulder_m / k` has `b = h`, `q = 1`: GP3's offsets of today.
  - Between a two-foot round's foot and its point (under the ring), a side keeps the foot's values.
- **Candidates (to the slice):**
  - per group, the union of its chains' one-sided strokes and its rings: `Owner::Terrain`, `Surface::Sidewalk` (code 9), `HeightRule::KerbOffset`, key `(rank sidewalk, 0, 0)`. A patch's `material` and tie-break come from the chain span nearest its inner point (GP3's `way_glob` rule), so two cells label a border patch alike whatever part of the group they load;
  - one mask per ground building (removes ranks);
  - one stop mask per stop arm on a chain (removes the guest's own candidates): the host's one-sided stroke at the pair's `stop_at` layer on that side, from the chain's points, plus the ring where the arm lies in a ring's sector;
  - one zebra region per holder.
- **Surfaces:** `Surface::Zebra` (code 10) on carriageway-type patches inside a zebra region (H7), with `material` the holder way, so GP11 reads the holder's tangent at any zebra patch, a junction-owned one included.
- **Edges:**
  - `EdgeKind::Kerb` on a sidewalk edge against a carriageway-type face (carriageway, crossing, zebra; a junction face) whose owner is the chain's way, a junction or joint of that way, or another road where the chain runs in;
  - `Continuous` on a sidewalk edge against a stop guest of the chain's way at its stop edge (flush, #3040 section 5);
  - `Continuous` with `UNRELATED` on every other sidewalk edge against a carriageway-type face.
- **`FacePlan.kerbs: Vec<KerbEdge>`**, sorted by edge: `KerbEdge { edge: u32, marks: Vec<KerbMark> }` for each `Kerb` edge, with `KerbMark { t: f32, rise_m: f32, front_m: f32 }` at `t` = 0 and 1 and at every breakpoint of `R` or `F` that lies on the edge (`t` along the edge from `a` to `b`). The values are linear between marks. The marks are positions, not vertices of the map: the slice gets no extra vertex, and no cut line.
- **Stats** (`FaceStats::counters()`): `walk_chains`, `walk_groups`, `walk_rings`, `walk_run_ins`, `walk_tapers`, `walk_short_chains` (shorter than two end lengths), `walk_drops`, `zebras`, `zebras_refused`, `building_masks`, `stop_masks`, `kerb_unrelated`.

GP4 writes no `KerbProfile`, `Taper`, `Drop`, `KerbId`, `Face.kerb`, `Face.marking` and no way frame.

## Rule block

The block `walk` in `data/trailview/plan.json`. No value lives in code.

| Field | Unit | Default | Why |
| --- | --- | --- | --- |
| `end_slope` | 1 | 1.0 | The back edge meets the kerb at 45° for every width (owner decision, from the sheet of 27° and 45°) |
| `end_round_m` | m | 1.0 | The fillet radius at each kink of the back edge (owner decision: round corners); shrunk where two fillets would overlap |
| `radius_one_walk_m` | m | 0.8 | The junction corner radius where exactly one facing side walks (owner decision 1, the orchestrator's reading: it overrides the class value; the owner may revise) |
| `kerb_lowered_m` | m | 0.0 | The step height of a lowered kerb: the epic says it falls to zero (approved) |
| `kerb_ramp_m` | m | 1.2 | A lowered kerb's ramp along the kerb (today's `RAMP_M`); never shorter than `kerb_ramp_m · drop_least_sine` across the drop's line |
| `drop_clear_m` | m | 0.5 | A lowered stretch is wider than the crossing way by this on each side (today's `DROP_CLEAR_M`) |
| `drop_least_sine` | 1 | 0.3 | A way at a shallow angle lowers the kerb as if it crossed at this sine (today's `LEAST_SINE`) |
| `drop_max_m` | m | 6.0 | The longest half of a lowered stretch: a track of 3 m half-width at sine 0.3 (`(3 + 0.5) / 0.3` = 11.7 m) is no real entrance (finding 14) |
| `cross_cos` | 1 | −0.5 | A path lowers the kerb where another path arm leaves the host's other side at this cosine or less (today's `RUNS_ON_COS`) |
| `over_buildings` | list | `["roof", "carport"]` | `building` values that stand over the ground |
| `zebra_markings` | prefixes | `["zebra", "ladder", "yes"]` | `crossing:markings` values that paint a zebra |
| `zebra_crossing` | values | `["zebra", "marked", "uncontrolled"]` | `crossing` values that paint one where `crossing:markings` is absent |
| `zebra_reach_m` | m | 2.5 | A node farther than this from every paved road's centre line has no zebra (today's `NODE_REACH_M`) |
| `zebra_half_m` | m | 2.0 | Half the zebra's length along the road (today's `ZEBRA_HALF_M`) |

Other rows of `plan.json`:
- `ranks`: `["carriageway", "rail_bed", "sidewalk", "shoulder_paved", "shoulder", "verge", "slope", "terrain"]`.
- `masks`: `{"building": ["sidewalk", "shoulder_paved", "shoulder"]}`: a ground building removes these ranks from the patches it covers (the epic: "a sidewalk, then a shoulder, gives way"; verge and slope stay, so GP6 builds no wall at a facade).
- `pairs`:
  - every row gains `kerb` (`"whole"`, `"lowered"` or `"across"`, default `"whole"`) and `stop_at` (`"shoulder"`, default, or `"sidewalk"`);
  - `road/track`: `kerb: "lowered"`; `road/path`: `kerb: "across"`; `road/steps`: `kerb: "whole"`;
  - new `road/driveway`: `mouth: "stop"`, `stop_at: "sidewalk"`, `kerb: "lowered"`, `blend: true` (its corner radius: the driveway's class, `service`, as every pair at bc4f0a8).
- `group_tags`: `{"driveway": {"service": "driveway"}}`: a way with these tags takes the group `driveway` where a pair with that group exists for its host; otherwise it keeps its class group (finding 11).
- The reach row `walk` (below).

Not in this design (from the first approved block): `corner_look_m` (no run-on), `end_m`, `least_run_m`, `least_m`, `clear_m`, `one_corner_m` (now `radius_one_walk_m`), `kerb_ramp_across_m` (GP6's), `zebra_on_cos`, `zebra_same_foot_m`, `frame_margin_m`, `frame_slack_m`.

## Algorithm, per cell and level 0

GP4 runs inside GP3's `faces::build`. Each step reads only the results of the steps before it.

1. **Radii** (before GP3's rounds; junctions only). At each junction, for each sector between neighbouring non-stop arms, count `n`, the facing sides that walk. The corner radius is `corner_radius_m` of the lesser class at `n = 0`, `corner_radius_walk_m` of the lesser class at `n = 2`, and `walk.radius_one_walk_m` at `n = 1` (owner decision 1, the orchestrator's reading). Joints and turns keep `inner_radius_m` (hard rule 3).
2. **Rounds** (GP3): two-foot and one-foot rounds with these radii; stop arms take no part in a sector's corner round (H6).
3. **Chains.** Per way side, the stretches. Then, at each graph point, per sector between neighbouring non-stop arms:
   - **Two-foot round, both walk:** cut each side between its foot and the point; the chain end at the foot keeps its width (no constraint); build a ring.
   - **Sector of half a turn or more, both walk:** join the two stretches into one chain through the point.
   - **One walks, or no two-foot round in a sector under half a turn:** where the other arm is another road, the walking chain runs on to P and ends there with no constraint. If P lies on the walking arm, cut the side between P and the point, unless the same road walks on the far side of that road. If P lies behind the point, the chain runs on along its road (its line, or the through partner arm at the point) to P. Where no P exists on the chain's road (a sector over about 120° at a split way, a sector of half a turn or more), the end has the target 0. Where the other arm is the same road, the chain ends at the foot or at the point with the target 0.
   - Every other end (a bit change, a way's end, a level change) has the target 0.
   A one-foot round splits nothing. A chain of zero length goes. Chain pieces keep their stations.
4. **Back edge and rise.** Along a chain, in run metres: `b(s) = min over the constraints (b_i + k · |s − s_i|)`, `w = max(0, b − h)`. Then each kink of `b` gets a fillet of radius `min(end_round_m, room)`, with the tangent length `T = R · tan(φ / 2)` for a turn `φ`. `room` keeps two fillets from overlapping. At a 0-target end, the cone's apex lies `T` before the end station, so `w` runs into 0 tangentially at the end station. The kerb stays straight: `b ≥ h` everywhere. `r(s) = w(s) / w_full(s)`.
5. **Chain strokes.** Each chain's point list is its pieces' samples in order, plus interpolated points (linear on the chord, their segment's direction) at the cone kinks and at the fillets' points at even steps of at most 10° of turn. GP4 builds the list after GP3's index state (arms, regions, seams); GP3's lines get no sample. The stroke is one-sided: its inner edge lies at `h/2`, inside the carriageway and off the centre line where GP3's fans run. A mitre point or a pie joins the segments, as GP3's strokes do, so no square end lies inside a chain.
6. **Offsets** to GP3: per side with a chain, the point list with `b` and `q`; GP3 strokes that side's band layers from it (H1). Other sides keep GP3's offsets.
7. **Rings.** At each two-foot round where both facing sides walk: an annulus sector about `c` between the feet's normals. Its inner arc runs at the global arc directions with the radius changing linearly by angle from `max(0, ρ − w_a)` to `max(0, ρ − w_b)` (`w` the chain widths at the feet). Its outer arc is `ρ + INSET`, inside GP3's fan. Its ends extend `INSET` units past the feet into the chains. The ring joins its chains' group candidate (one union), so no edge is shared by two operations.
8. **Stops** (H6). A road arm with the `group_tags` takes the group `driveway` where its host has that pair. Each stop arm gets a stop mask: the host's one-sided stroke out to the pair's `stop_at` layer on the side the arm meets, from the same points as that layer (plus the ring where the arm lies in a ring's sector). Inside the mask, the guest's own candidates do not count; the patch takes the next key, which is the host's sidewalk or the host's shoulder. So the guest ends exactly on the host's stop edge, and the host's chain runs on. A junction region does not relabel a stop guest's patches, so the guest ends in its own surface (the epic: "a small corner fill in its own surface"). GP3 builds the stop rounds after step 6, tangent to the stop edge, in the guest's label.
9. **Drops.** A drop is a strip about a line: `origin`, unit `dir`, half-width `a`, ramp `e`, and a length along the line. A kerb point's weight is `u = clamp((a + e − dist) / e, 0, 1)` where its foot on the line lies within the length.
   - A stop arm of a pair with `kerb: "lowered"`; or with `kerb: "across"` where another stop arm leaves the host's other side at a cosine of `cross_cos` or less, or where its way has `footway=crossing`: line = the arm's centre line from the point; `a = min(drop_max_m · |sin θ|, (h_g + drop_clear_m) · min(1, |sin θ| / drop_least_sine))`; `e = kerb_ramp_m · max(|sin θ|, drop_least_sine)`; length `h_host + w_max + kerb_ramp_m`. Measured along the kerb, the stretch is `(h_g + drop_clear_m) / max(|sin θ|, drop_least_sine)` (at most `drop_max_m`) plus ramps of `kerb_ramp_m`, today's rule.
   - A zebra: line through the node, square to the holder; `a = zebra_half_m`; `e = kerb_ramp_m`; length `h_holder + w_max + kerb_ramp_m`.
   - `F = R − u · (R − min(kerb_lowered_m, R))`, with `u` the largest weight. So a drop lowers every kerb point in its strip: on a chain, on a ring, and on the edge where a chain runs into a side road (owner decision 6).
10. **Zebras.** For each crossing point whose tags say a zebra: the holder is the paved road way whose centre line passes nearest the node within `zebra_reach_m`, and the ways that continue it at a graph point within `zebra_half_m` of the node's foot (finding 21). A node with no holder is refused and counted. Each holder gives a zebra region: its carriageway stroke between its stations `s_n ± zebra_half_m`. After the labels (H7), every carriageway-type patch (way or junction label) inside a zebra region takes `Surface::Zebra` and the holder as its material, as GP3 makes a carriageway over a rail bed a crossing. So a zebra shows at a footway node inside a junction face too (finding 3).
11. **Buildings.** Each ground building is a mask candidate (H7).
12. **Slice, labels** (GP3), then the zebra rule and the edge kinds.
13. **Hand-off.** Edge kinds as in "Data that GP4 writes". For each `Kerb` edge, marks at its two ends and at every breakpoint of `R` (the taper's start, its fillet points) and `F` (each drop's `a` and `a + e`) whose kerb point lies on the edge, each with:
   - `R = kerb_m · r` at the station of the nearest chain kerb line (projection clamped to the chain; ties go to the lesser global key of the span the projection lands on, finding 16);
   - on a ring's arc, `R` is the linear interpolation by angle between the two feet's values;
   - `F` comes from the drops by position. So `F` is continuous round a ring (finding 9c).
   A sidewalk edge between two faces of one group is `Continuous`; between two groups it is `Continuous` with `UNRELATED`.

## Invariants

1. GP4 moves no carriageway, junction or zebra edge, except one thing: the corner radius of a junction comes from the walk count (a table value). The sidewalk ranks under every carriageway-type face, and its stroke starts inside the carriageway.
2. A sidewalk lies only on a chain or a ring of a two-foot round where both facing sides walk. So the viewer never decides again, and no sidewalk curves into a corner without one.
3. A chain end runs into another road across it, or it has the width 0 at its end station and meets the kerb tangentially. The slab is never cut square in the open. Where a slab meets a rail bed, a foreign carriageway or a building, the overlay cuts it by rank or mask: intended (owner's end rule), not a fault.
4. A two-walk corner's sidewalk is one region: the ring overlaps both chain ends.
5. A stop guest never cuts a chain.
6. Every candidate is a valid polygon set with positive winding; the slice and the keys decide every overlap, so the plan is a planar partition for every input.
7. `0 ≤ F ≤ R ≤ kerb_m` at every kerb mark; `R = 0` at a taper's tip.
8. Every value comes from global inputs (below).

## Case distinctions that remain

1. **Stop arms take no part in sectors** (steps 1 to 3). Reason: the owner's rule "the host's sidewalk runs on"; the pair's `mouth` value decides.
2. **A chain splits only at a two-foot round** (step 3). Reason: a ring needs two feet; a one-foot round only rounds a carriageway corner.
3. **A join only across half a turn or more** (step 3). Reason: under half a turn, a joined slab passes the other arm's far kerb when the sector is under about `acos(h / (h + w))` (finding 5).
4. **An end against another road runs in (no constraint); an end against nothing has the target 0** (step 3). Reason: the owner's end rule.
5. **Same road or another road** (step 3), read from GP2's hosts. Reason: the same rule; a joint or the through pair is "nothing there".
6. **P behind the point, or no P** (step 3): run on along the road to P; with no P on the road, the target 0. Reason: geometry; an end with no road across it runs against nothing, so it tapers (owner's end rule).
7. **The junction radius column by the walk count** (step 1), at junctions only. Reason: the owner's approved radii; joints and turns keep hard rule 3.
8. **The kerb kind: `Kerb`, flush at a stop mouth, or `UNRELATED`** (step 13). Reason: rule 6 and #3040 section 5.
9. **The drop kind from the pair's `kerb` value and `footway=crossing`** (step 9). Reason: the owner's rule.
10. **Two OSM zebra schemes; ground building or not** (steps 10, 11). Reason: OSM tagging.
11. **The ring's inner radius clamps at 0** (step 7). A clamp, not a shape.

## Determinism

- Inputs: GP2's samples, stations and span lengths; `walk` bits on vertices; GP3's rounds; node positions; building rings in tile units. `libm` for `atan`, `tan`, `sin`, `cos`, `sqrt` of the fillets.
- `b`, `r`, `q` read run metres between stations only. A point's values depend only on constraints within `w_max / k + 2 · end_round_m` (4 m), on P within `extent_m`, and on drops within their strips.
- The chain strokes' interpolated points and the kerb marks come from stations and the chord of two global samples, so two cells make the same points. GP3's lines get no new sample.
- Kerb values come from position (nearest kerb line, drop strips), ties by global span keys.
- Sidewalk labels: `material` and the tie-break come from the chain span nearest the patch's inner point, so they read only data near the patch; check 2 compares that chain's global span key at each border segment.

## Reach

The row `walk` (owner `faces`) joins GP3's `faces input` and `faces cell` rows. Its terms: `w_max / k + 2 · end_round_m` (4 m), `drop_max_m + kerb_ramp_m` (7.2 m), `zebra_reach_m + zebra_half_m + kerb_ramp_m` (5.7 m), and P's reach `(h_max + w_max / 2) · |cot|`, capped at GP3's `extent_m` (20 m). The largest is 20 m. GP3's `band_m` reads `max(shoulder_m, sidewalk_m)` (12 m instead of 10.6 m). So `faces input` is 67.5 m and `faces cell` 137.5 m, under 191 m.

## Interfaces

**GP2 gives** `Sample::walk`, stations, span lengths, graph points, arms and hosts. Nothing new.

**GP3, the hooks** (as few and as small as possible; GP3 is still changing):
- **H1, band strokes per side.** On a side with a chain, GP3 strokes its band layers from GP4's point list with `b` and `q` (`pieces_along` with a left and a right offset list); elsewhere, from its samples as today. `cuts`, `fill_gaps` and the band rings read the side they face. This is the one large hook.
- **H2, rounds out.** Per graph point, per sector: the two-foot round (centre, radius, foot stations) or the one-foot round's circle. GP3 has them inside `round`; the hook exposes them.
- **H3, junction radius by walk count:** GP3's "either facing side walks" flag (`sidewalk`, bc4f0a8) becomes the count `n` of walking facing sides, and `n = 1` reads `walk.radius_one_walk_m`.
- **H4, extra candidates:** GP3's slice takes GP4's candidates with their own labels, keys and the nearest-chain rule for `material` and tie-break.
- **H5, ranks and surfaces:** the rank `sidewalk`, `Surface::Sidewalk`, `Surface::Zebra`.
- **H6, stops** (GP3 scope, not built). Corner rounds pair neighbouring non-stop arms. A stop mask removes the guest's own candidates inside it; a junction region does not relabel a stop guest's patches; the stop rounds come after the offsets.
- **H7, two label rules:** a mask candidate removes its listed ranks (building) or its guest's candidates (stop) from the patches it covers; after the labels, a zebra region turns carriageway-type patches into `Zebra` with the holder as material (as the rail-crossing rule).
- **H8, output:** GP4 sets the sidewalk edge kinds and fills `FacePlan.kerbs`.
- **H9, reach:** `band_m` and the `walk` row.

**GP3 gives, beyond the hooks:** a slice that stays fast where two outlines cross at a small angle near a vertex. The probe's slow cell (34296, 23098) is one 3 m sidewalk quad whose back edge crosses a junction fan's side at 3.9°, about 5 cm from the quad's corner: the slice goes from 30 to 760 ms with 4 more segments (i_overlay solver passes; the reviewer's measurement). GP3 has the same hazard without sidewalks; the fix belongs in GP3's slice. GP4 keeps the per-cell gate (no cell over 3× its GP3 time).

**GP5 takes** `Surface::Sidewalk`, `Surface::Zebra`, `EdgeKind::Kerb`, the `UNRELATED` flag and `FacePlan.kerbs` into `Flat` and the canonical bytes.

**GP6 takes** the `Kerb` edges and `FacePlan.kerbs`:
- At a sidewalk point, the nearest `Kerb` edge of its own face gives `z`, and `R` and `F` linear between that edge's marks, and the distance `d`. The height is `z + F + (R − F) · min(1, d / a) + cross-fall`; `a` is GP6's `kerb_ramp_across_m`.
- A face's kerbs belong to its group, so they are related; two groups meet at an `UNRELATED` edge (rule 6).
- A stop mouth is `Continuous`: the guest's edge takes the slab's height (flush).
- The shoulder that starts past a chain end meets nothing raised: on the chain `q = 0`.
- GP4 builds no way frame.

**GP7 takes** the `Kerb` edges as step edges and puts a mesh vertex at every interior mark of `FacePlan.kerbs`, so the step's height changes where `R` and `F` change. Where `F = 0` (a drop with `kerb_lowered_m` 0, a taper's tip), the step has height 0: GP7 draws no vertical face there and meshes the edge as continuous. A taper's tip runs tangentially into the kerb. The shoulder past an end starts at 45°, not at a thin angle.

**GP11 takes** the sidewalk faces (slab look), the `Kerb` edges (kerb stone, `kerb_top_m` of the class), the sidewalk's back edges (edging stone), and the `Zebra` faces (stripes across the tangent of the face's material, the holder way, junction-owned patches included).

**GP17 takes** the back edges for lamps: sidewalk edges against ground faces (shoulder, verge, slope, terrain), not `Kerb`, not `UNRELATED`, not a stop mouth, not a building mask edge, and not within a round's circle (`FacePlan.corners`).

## Edge cases

- **No `walk` bit:** no chain, no sidewalk; GP3's offsets stay; no kerb mark.
- **A chain across a cell line:** every value reads stations within the `walk` reach; the strokes' points are global.
- **Short chains:** a chain shorter than two end lengths is a low diamond with round corners. It is valid, and it is counted (`walk_short_chains`).
- **Unequal widths at a joint (class change), and a one-foot round there:** the joint is one chain; the back-edge cone narrows the wider slab at 45° with round corners; the kerb follows GP3's carriageway and its one-foot round. No step in the back edge (finding 1).
- **Two-walk corner, `ρ < w`:** the ring's inner edge clamps at `c`; the chain ends overlap behind it in a sharp back corner.
- **Two-walk sector under half a turn with no two-foot round:** both chains run into the other arm, each to its P; the sector joins them in one group, and the slice gives each patch its nearest chain.
- **Acute one-walk sector:** P lies on the walking arm; the end lies inside the side road's carriageway at every offset because `w_max / 2 ≤` the least half-width of a road (1 m ≤ 2 m). The reader checks this condition.
- **Obtuse one-walk sector:** P lies behind the point; the chain runs on along its road (its line or its through partner) to P. Where the road ends at the point and no P exists, the end tapers (target 0): no square end in the open.
- **Close junctions:** feet that cross give a chain piece of zero length; it goes; the rings overlap in one group.
- **Tight bends:** a turn's round at the inner side splits the chain where both facing sides walk and carries a ring; the bake bars most of them.
- **Hairpin legs or parallel roads whose slabs touch:** two groups, two faces, an `UNRELATED` edge between them; the other leg's carriageway wins over a slab.
- **Roundabouts:** the ring road's outer side is one chain; each side road gives two junction sectors.
- **A driveway on a side without a sidewalk:** `stop_at: sidewalk` has width 0 there, so it reaches the carriageway edge with its class's round (owner decision 4).
- **A path or a driveway at a taper:** its stop mask is the host's stroke at `b(s)`, which narrows with the taper. The guest meets the narrower slab.
- **A track at a side without a sidewalk:** its stop mask is the host's stroke to the shoulder's outer edge; inside it the track's own candidates do not count, so the host's shoulder shows, not the track's own shoulder.
- **A path at a two-walk corner:** it stops on the ring's back edge (H6); the ring runs on (unit test).
- **A drop near a corner:** the strip lowers the chain's and the ring's kerb points it covers, by position.
- **A path across the road at one node:** two stop arms on the two sides: two drops. A path that ends: none, unless `footway=crossing`.
- **A zebra at a footway node inside a junction face:** the zebra rule turns the junction's patches in the region into `Zebra`. Two zebras within 4 m: their regions overlap, one zebra face.
- **A crossing node on a path only, or off every paved road:** refused, counted.
- **A building over the carriageway:** the carriageway keeps its place; the mask removes the slab and the shoulders under the building, and verge, slope or terrain remain there.
- **A roof, a carport, an arcade:** no mask.
- **A level crossing:** at a shared-node crossing the bake keeps `walk` (its bed rule skips 12 m about the node); by the owner's end rule the slab runs into the rail bed, and the bed, which ranks over it, cuts it.
- **A bridge, a tunnel, a ford:** no `walk` there; a chain ends at the level change with the target 0.
- **Duplicate or zero-length segments:** GP2 removes them.

## Plan

1. GP3 hooks H1 to H9, on GP3's branch or in GP4's pull request where GP3 left them out, with GP3's tests green.
2. `host/obc-ground-plan/src/faces/walk.rs`: step 1 (walk count), chains (step 3), back edge, rise and fillets (step 4), chain strokes (step 5), offsets (step 6), rings (step 7), groups, building masks (step 11).
3. `host/obc-ground-plan/src/faces/kerbs.rs`: drops (step 9), stop masks (step 8), edge kinds and the kerb marks (step 13).
4. `host/obc-ground-plan/src/faces/zebras.rs`: holders and regions (step 10).
5. `data/trailview/plan.json` and its reader (`faces/rules.rs`), with the check `w_max / 2 ≤` the least road half-width.
6. `faces/check.rs`: the GP4 checks.
7. The debug tool: colours for sidewalk and zebra, kerb edges by `F`, the GP4 checks and stats in `--faces-bench`, and the sheets `walk-ends` (tapers and run-ins), `walk-corners`, `drops`, `zebras`.
8. If GP5 merged first: `Flat` and its bytes.
9. Tests.

## Files

- `host/obc-ground-plan/src/faces/walk.rs`, `kerbs.rs`, `zebras.rs`: create.
- `host/obc-ground-plan/src/faces/mod.rs`, `points.rs`, `stroke.rs`, `rules.rs`, `types.rs`, `check.rs`, `tests.rs`: change.
- `data/trailview/plan.json`: change.
- `host/obc-ground-plan/examples/debug/` and its `README.md`: change.
- `host/obc-ground-plan/src/transport.rs` (GP5): change if GP5 merged first.

## Verification

- **Checks over every fixture cell** (`obc test -p obc-ground-plan` and `--faces-bench`):
  - Check 1 and check 2: no partition fault and no border difference; 0 artefacts on sidewalk and zebra faces and their edges; no increase of GP3's artefact counts against GP3's head built with the same radii. Check 2 compares the kerb marks at border vertices and the nearest chain's global span key of each border patch.
  - `kerb_moved`: the carriageway-type area differs from the same build without GP4's candidates, masks and zebra rule. Inside a stop guest's mask (H6) the difference is the guest's stop and counts apart.
  - `walk_stray`: a sidewalk patch whose inner point lies farther than `w + 1 cm` from every chain's kerb line and every ring. It catches a curve into a corner without a sidewalk.
  - `walk_sector`: a sidewalk patch of a point's chains inside a sector of that point where no facing side walks, within `extent_m` of the point.
  - `end_shape`: at each 0-target end, the slab is wider than 1 cm at its end station, or not at full width at `w / k + 2 · end_round_m` from it.
  - `kerb_kind`: a sidewalk edge against a carriageway-type face whose kind does not follow step 13; a `Kerb` edge without those faces or without marks; a mark with `F > R` or `R > kerb_m`.
  - `ring_open`: at a two-walk two-foot round, the two chain ends do not lie in one sidewalk face (a mask over a foot excludes it, counted).
  - `mouth_broken`: at a stop arm on a chain, the points at `h + w/2` at the arm's foot and 0.5 m past its drop on both sides do not lie in sidewalk faces of one group (masks excluded, counted).
- **Unit tests** (`faces/tests.rs`, one per case):
  - a straight road whose bit ends: kerb straight; `w = 0` and `R = 0` at the end with a tangential tip; full width at `w / k + T`; no shoulder behind the slab, a 45° shoulder start past the end;
  - a T where both walk: one face round the corner, the ring overlapping both chains;
  - a T where one walks: the slab at full width up to the side road's carriageway; no sidewalk in the sector; the kerb on the run-in edge with `R = kerb_m`;
  - the same at 30° and at 150°;
  - an acute two-walk sector with no round;
  - a class-change joint with a one-foot round: one chain, a 45° narrowing, no back-edge step;
  - a path at a two-walk corner: the path stops on the ring's back edge in its own surface (no junction relabel);
  - a track at a host side without a sidewalk: it stops at the host's shoulder edge, and the host's shoulder shows inside the mask;
  - a driveway: the chain runs on; the driveway stops on the back edge, flush; `F = kerb_lowered_m` over the stretch, ramps of 1.2 m;
  - a path across at one node (two drops), a path that ends (none), a `footway=crossing` path that ends (one);
  - a zebra on a plain road, and a zebra at a footway node inside a junction face;
  - a level crossing: the slab runs into the bed;
  - a ground building at the kerb: no slab and no shoulder under it, a verge or slope there;
  - a chain across a zoom-13 line from two tiles: equal points, equal kerb marks on the border.
- `obc exec cargo clippy -p obc-ground-plan --all-targets -- -D warnings`; `cargo fmt --all`; `obc suites check` if `suites.toml` changes.
- **Measurements:**
  - Time: `--faces-bench` on GP3's head and on GP4's head, the same cells, alternating runs on an idle machine. Gate, separately for town and rural cells: the median of GP4's added time at most 25 % of GP3's median, its p95 at most 50 % of GP3's p95, and no cell over 3× its GP3 time. Reason: the epic leaves the build gate to GP7, and the owner sets gates per cell type (GP3 review point 6). The probe's slow cell is GP3's slice hazard ("Interfaces").
  - Faces, vertices and slice segments per cell before and after, for GP7.
  - Stats summed over the fixture, among them `walk_short_chains`, `walk_run_ins` and `stop_masks`.
- **Pictures** (debug tool, top-down, before and after): `v09-sidewalk-patches-top`, `v11-two-bad-junctions-top`, `v12-good-town-junctions-top`, `v17-sharp-corners-top`, `zebra-tagged-top`, `crossing-close-top`, `conn-own-a-top`, `conn-own-b-top`, `conn-own-c-top`, `conn-path-off-walk-top`, `conn-walk-into-path-top`, `conn-walk-into-track-top`; the fixture sheets `walk-ends`, `walk-corners`, `drops`, `zebras` (all, or the 60 with the least room and 20 at random). The implementer looks at each.

## Done when

- Sidewalks, kerbs, drops, zebras and building masks are in `faces::build` for every fixture cell.
- Check 1 and check 2 as above and the GP4 checks give 0 over the fixture.
- The unit tests pass; clippy and fmt are clean.
- The time gate holds; the pull request gives the stats and the face counts.
- The pull request has the pictures and the sheets, and the implementer looked at each.
- `plan.json` holds every value.

## Out of scope

- Heights, cross-fall, the kerb's vertical face: GP6. The mesh: GP7. Looks: GP11. Decks: GP12. Level crossings: GP13. Lamps: GP17.
- Separately mapped sidewalks (`footway=sidewalk`): they stay paths.
- The bake rule for sides whose bits start a few metres short of a junction, and its fixture rebake: a follow-up in GP18's area (owner decision 7).
- The viewer's old sidewalk code: GP19.

## Owner decisions

1. One-walk corner radius: 0.8 m; the slab runs at full width straight to the side road's carriageway edge. The orchestrator reads this as overriding GP3's per-class walk radius where exactly one facing side walks (`walk.radius_one_walk_m`); the owner may revise.
2. Ends: a sidewalk tapers only where it runs against nothing (a `walk` change along a road, a free end). Where it meets a road, it keeps its full width and runs in; the overlay cuts it along that road's carriageway edge. Short chains stay and are counted. Tapers are steeper and have round corners (`end_slope`, `end_round_m`).
3. Buildings: the slab ends flat on the facade. Proposed epic sentence (section 5, "Buildings"): "A sidewalk, then a shoulder, gives way where a ground building stands in its band; the cut follows the facade, whose wall hides the end. A carriageway never gives way."
4. Driveway on a side without a sidewalk: one rule (`stop_at: sidewalk`).
5. End slope: one angle for every width: 45° (`end_slope` 1.0) with round corners of 1 m (`end_round_m` 1.0), chosen from the sheet of 27° and 45°.
6. Kerb at the slab's end along a side road: a normal kerb, lowered only where a path or a zebra crosses there.
7. Sides whose `walk` bits start 2 to 6 m from a corner (11 on the fixture): the bake fixes them (a follow-up in GP18's area: a bake rule that runs such a sidewalk on to the junction, and a fixture rebake). GP4 builds no run-on.

Proposed epic sentence for section 5, "Sidewalk corners and ends": "Where both arms of a corner have a sidewalk on that corner, the sidewalk follows the kerb round the corner. Where a sidewalk meets another road, it keeps its full width and runs in; the overlay cuts it along that road's carriageway edge. Only where it runs against nothing does it end with a taper: the kerb stays straight, the back edge converges to it with round corners, the kerb's rise falls to zero, and the ground fills behind."

No question is open.

## Review response

Round 1 (xhigh review of the first redesign draft):

| # | Finding | Resolution |
| --- | --- | --- |
| 1 | One-foot rounds break chain, ring and kerb rules; back-edge jump at a width change | Fixed: a chain splits only at a two-foot round (case 2); the cone acts on the back-edge offset `b(s)`; every round's arc is a kerb line |
| 2 | Stop mouth unbuilt and out of order; path in a two-walk sector; junction relabel behind the slab | Fixed in H6 (refined in round 2: stop masks, no relabel); corner rounds skip stop arms; stop rounds after the offsets; unit test "path at a two-walk corner" |
| 3 | Zebras vanish in junction regions; key claim false | Fixed: a post-label surface rule (H7), no key slot; unit test at a footway node |
| 4 | Walk-count radii at joints break rule 3 | Fixed: junctions only; joints and turns keep `inner_radius_m` |
| 5 | Joins through acute sectors spill past the far kerb | Fixed: join only across half a turn or more; narrower sectors without a round run each chain into the other arm (owner's end rule) |
| 6 | Ring width and chain-end width not tied | Fixed: the ring takes each foot's width (radius linear by angle), overlaps the chain ends and joins their union; no target at a foot |
| 7 | `corner_look_m` on a false cause | Fixed: the cause is not the bed rule (`JOIN_M`); fixture count 398 first-segment against 11 at 2 to 6 m; look and run-on dropped; owner decision 7 (bake fix) |
| 8 | Slab against a rail bed or foreign carriageway | By the owner's end rule this is intended: it runs in and is cut by rank; invariant 3 says so |
| 9a | One label merges unrelated slabs | Fixed (refined in round 2): material from the nearest chain; groups for relation |
| 9b | Stop-mouth edges become `UNRELATED` | Fixed: flush `Continuous` at a stop mouth of the chain's host; `Kerb` and `UNRELATED` defined (step 13) |
| 9c | `F` jumps on a ring at the bisector | Fixed: drops act by position; `R` on a ring is interpolated by angle |
| 9d | GP17 back edges | Fixed: the edge filter in "Interfaces" |
| 9e | GP7 and zero steps | Fixed: `F = 0` is a step of height 0, meshed as continuous |
| 10 | Sample insertion breaks GP3's index state and adds jogs | Fixed: no insertion into GP3's lines; chain strokes carry their own points (round 2: the kerb breakpoints became marks) |
| 11 | `group_tags` sends pairs to `default` | Fixed: the tag group only where a pair with it exists |
| 12 | Check 1 includes GP3's open artefacts; smoke checks; missing tests | Fixed: GP4-only artefact targets and "no increase"; `kerb_moved`, `end_shape`, `walk_sector` replace the rank-only checks; the five tests added |
| 13 | Bands between a two-walk foot and the point | Fixed: the side keeps the foot's values there |
| 14 | Drop length unbounded | Fixed: `drop_max_m` |
| 15 | Path at a taper: step and edge case disagree | Fixed: the stop mask follows `b(s)`; both texts say so |
| 16 | Tie-break key | Fixed: the key of the span where the projection lands |
| 17 | Buildings over verge and slope; filter | Fixed: masks remove sidewalk and shoulders only; filter by chain and shoulder boxes |
| 18 | Paved shoulder in the taper wedge | Fixed: `q = 0` on a chain; past an end the shoulder starts at the end slope; GP7 told |
| 19 | Invariant 1 wording | Fixed |
| 20 | `footway=crossing` dropped | Fixed: kept (step 9) |
| 21 | Zebra holders too wide | Fixed: the nearest paved road and its continuation at the node |
| 22 | Picture index overclaim; time gate at risk; hook size | Fixed: index corrected; a per-cell 3× bound added; hooks counted, size L |

Round 2 (delta review; the owner decisions are fixed):

| # | Finding | Resolution |
| --- | --- | --- |
| 1 | Band strokes see `b`, `q` only at GP3's samples: a spacing-dependent shoulder strip and near-coincident edges | Fixed: on a side with a chain, GP3 strokes the band layers from the chain's point list (H1); the carriageway keeps its samples |
| 2 | Cut lines end inside faces and leave slits | Fixed: no cut lines; the breakpoints are marks on each kerb edge (`KerbEdge`, `KerbMark`), linear between; GP7 puts a mesh vertex at each mark |
| 3 | One shoulder candidate for all ways: a track loses to its own shoulder | Fixed: a stop mask per stop arm removes only that guest's candidates; the host's stroke to `stop_at` from the same points |
| 4 | The junction region relabels the stop guest behind the stop edge | Fixed: a region does not relabel a stop guest's patches; the guest ends in its own surface |
| 5 | Group label depends on what a cell loads; group definition contradiction | Fixed: `material` and tie-break from the chain span nearest the patch's inner point (`way_glob` rule); a group includes chains that run into each other in one sector |
| 6 | Radius rows stale against bc4f0a8 | Fixed: n = 0 `corner_radius_m`, n = 2 `corner_radius_walk_m`, n = 1 `walk.radius_one_walk_m` (owner decision, orchestrator's reading) |
| 7 | Square end in the open where no P exists | Fixed: run on along the same road (through partner) to P; no P, target 0 |
| 8 | Reach row's largest term is 20 m | Fixed: input 67.5 m, cell 137.5 m |
| 9 | Drop ramp at shallow angles | Fixed: `e = kerb_ramp_m · max(|sin θ|, drop_least_sine)` |
| 10 | GP11 cannot find a junction-owned zebra's tangent | Fixed: a zebra patch's material is the holder |
| 11 | `walk_missing` holds by rank | Dropped; `mouth_broken` covers the case |
| — | Level crossing sentence false at shared-node crossings | Fixed: the slab runs into the bed and the bed cuts it |
| — | Slow probe cell | Cause replaced by the reviewer's measurement (a small-angle crossing near a vertex in GP3's slice); listed under "GP3 gives"; the 3× per-cell gate stays |

## Probe

Branch `scratch/gp4-probe` (throwaway, base `claude/gp3-faces-v3` at 593a5b4). Pictures on the agent machine in `gp-pictures/gp4/design/` (`probe/`, `probe2/`, `INDEX.md`). The probe built chains through open sectors, full-width run-ins at P against other roads, tapers only at bit changes and free ends with 45° and 1 m fillets, and rings at two-walk rounds. It showed:
- the joint at `zoom-zebra-ring` is fixed: the cause was the probe's open-sector test with the arms reversed, plus two square ends at a bend;
- the one-walk ends run straight into the side road;
- the former wedge at `zoom-v12-north` runs in at its bottom;
- the ring's step at a width change (finding 6, fixed in the design, not in the probe).

It did not build stops, drops, zebras, masks, per-foot ring radii or the cone on `b(s)`. Its bench (under load from other agents) shows 0 partition faults and 0 border differences; one cell's slice is GP3's small-angle hazard ("Interfaces").

