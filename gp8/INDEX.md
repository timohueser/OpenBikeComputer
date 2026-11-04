# GP8 pictures: the far stamp (first set)

Viewer built from commit 796e474 of `claude/gp8-far-stamp` (the far stamp; later commits change only
the audit, the tests and the zero-length test of a segment). SwiftShader (no GPU), 1920 × 960, one
view per page. `-plan` = `&ground=plan` (the far stamp, no snap, no corridors). `-old` = without the
switch (today's stamp, snap and corridors). About 11 minutes per view on this machine.

| Picture | What it shows | Does it look right |
| --- | --- | --- |
| i07-hairpin-top-plan.png | hairpin from above, far stamp | Yes: each leg keeps its own carriageway, the inner side of the hairpin is one shape, the line ends are round, the slope faces show as dark bands. No sidewalks (expected). |
| i07-hairpin-top-old.png | the same, old path | Reference: corridors with sidewalks, junction corner arcs. |
| t-tertiary-top-plan.png | T junction of the tertiary from above | Mostly: sharp junction corners, a round cul-de-sac end (top left), edge and centre lines run on (the centre line is solid, not dashed: the dashes come from the corridors). A small jag in the edge line at the side road's mouth (x 190, y 300). |
| t-tertiary-top-old.png | the same, old path | Reference: dashes, zebras, give-way marks and sidewalks come from the corridors. |
| junction0-top-plan.png | junction0 from above | Yes: the guest joins the through road with sharp corners, no blob, no notch; a slight kink on the guest's outer edge at x 845, y 320. |
| junction0-top-old.png | the same, old path | Reference: round corner arcs and sidewalks. |
| mouth-stream-top-plan.png | a stream's mouth at a road, from above | No: the track that leaves the hairpin to the right and the path across the meadow are missing (the old path paints both); to investigate. The outer edge of the bend (x 1270-1300, y 780-860) is jagged in steps of about one mesh cell. The stream ribbon stops at the road, as before. |
| mouth-stream-top-old.png | the same, old path | Reference: the track and the path show. |
| main-road-15m-plan.png | main road from 15 m | Partly: road and slopes are right, but the edge lines and the centre line wobble by up to a mesh cell (no mesh vertex lies on the road's edge without the snap). No guardrail (corridors off, expected). |
| main-road-15m-old.png | the same, old path | Reference: straight lines, guardrail, dashes. |
| valley-eye-plan.png | road at eye height | No: near the eye the far paint is coarse; the centre line wobbles and jumps sideways once (x 880, y 550); the side road on the left is a flat grey band. The old path draws corridors here; GP9's mesh replaces this view's near field. |
| valley-eye-old.png | the same, old path | Reference: the corridor mesh near the eye. |
| stream-lake-far-plan.png | the valley from far | Yes: ways read as thin lines; river, ponds and town as on the old path. |
| stream-lake-far-old.png | the same, old path | Reference; no visible difference at this distance. |
| valley-junction-top-plan-before-mitre-fix.png | the valley-eye junction from 20 m above, before the mitre fix | No: the centre and edge lines jog by about 0.2 m at two mitre lines (cause: alignment-tangent mitres, fixed in b6345e8, not shot again); the guest road paints a wedge into the through road up to the bisectors. |
