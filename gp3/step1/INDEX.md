# GP3 step 1: junction corner radius by class

Rule (proposed): a junction corner takes the lesser `corner_radius_m` of its two arms' classes
(`roads.json`), or the lesser `corner_radius_walk_m` where either arm has a sidewalk on the corner's
side of its first segment. A round still shrinks to the largest radius that fits. Joints and a way's
own turns keep `inner_radius_m`.

Every tile: 50 m square, 10 px per metre, north up, layers `faces,graph`. Left column CURRENT
(8d2464c7a: pair radii, road/road 7 m everywhere), right column PROPOSED (branch head). Blue circle:
a round with its centre; purple: junction face; grey: carriageway; tan: shoulder and slope; white:
terrain; magenta ring: an artefact of check 1; orange ring: a kink. The label lists the arms of the
junction nearest the tile centre (`*` = host) and the rounds whose centre is nearest that junction.

| Picture | What it shows | Does it look right? |
| --- | --- | --- |
| `town-1.png` | t-tertiary, x-tertiary, zebra-tagged, v12 (3 junctions), v13 | Yes. Town corners go from 7 m to 3 to 5 m; most town corners have a sidewalk bit, so the walk radius (3 m for residential and unclassified) decides. |
| `town-2.png` | v16, junction0 (j0), conn-parallel-way, b45, i07-hairpin, v14 | Mostly. v16 4 m (no sidewalk), j0 3 m. i07: the left junction's north-west round is 1.6 m (3 m asked) where the current gives 4.3 m (7 m asked): the fit search is not monotonic. b45 does not change (its rounds were already shrunk). |
| `rural-1.png` | v11 (2), v08 (2), v06, v07 | Yes. Only road/road corners change (v11-a 7 to 3 m, v06 4.7 to 3 m); track 3 m and path 1 m stay. |
| `rural-2.png` | j-tt-a, hp, d-hairpin, conn-track-off-road, v04 | Yes. No change except d-hairpin's inner round (5.2 shrunk to 5.0 asked). |
| `kinks-changed.png` | Places where the kink count changed: new1 to new3 (new kinks), gone1 and gone2 (kinks gone) | Honest: new1 to new3 lose a round at 3 to 4 m that fitted at 7 m (the same non-monotonic fit search); gone1 and gone2 gain a round. A fault of the fit search (open item 1), not of the values. |
| `current/faces-bench.json`, `proposed/faces-bench.json` | The fixture bench (256 cells, best of 3 runs, 4 threads, 3 GB cap, this machine) | See the table in the report. |
