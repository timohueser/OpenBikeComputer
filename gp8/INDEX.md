# GP8 pictures: the far stamp (first set)

Branch `claude/gp8-far-stamp` at 63c589f. SwiftShader (no GPU), 1920 × 960. `-plan` = `&ground=plan`
(the far stamp, no snap, no corridors); `-old` = without the switch (today's stamp and corridors).
About 11 minutes per view on this machine.

| Picture | What it shows | Does it look right |
| --- | --- | --- |
| i07-hairpin-top-plan.png | hairpin from above | Yes: each leg keeps its own carriageway, the hairpin's inner side is one shape, line ends round, slope faces visible as dark bands. |
| t-tertiary-top-plan.png | T junction of the tertiary from above | Mostly: sharp junction corners, a round cul-de-sac end (top left), centre and edge lines run on. A small jag in the edge line at the side road's mouth (left, near x 190, y 300). |
| junction0-top-plan.png | junction0 from above | Yes: the guest joins the through road with sharp corners, no blob, no notch. |
| mouth-stream-top-plan.png | stream mouth at a road, from above | Mostly: the stream ribbon stops at the road. The outer edge of the road's bend (right, x 1270-1300, y 780-860) is jagged in steps of about one mesh cell. |
| main-road-15m-plan.png | main road from 15 m | Partly: the road and its slopes are right, but the edge lines and the centre line wobble by up to a mesh cell. Expected without the snap: no mesh vertex lies on the road's edge. |
| valley-eye-plan.png | road at eye height | No: near the eye the paint is coarse; the centre line wobbles and has one sideways jump (x 880, y 550); a side road on the left shows as a flat grey band. Near the eye the old path draws corridors; GP9's mesh replaces this. |
| stream-lake-far-plan.png | valley from far | Yes: roads are thin lines, river and ponds as before. |
