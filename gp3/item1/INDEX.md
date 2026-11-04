# GP3 item 1: kinks and fragments (state at 215e990)

Before: bc4f0a8 (step 1, proposed class radii). After: 215e990 (head). Tiles 20 m at 18 px/m (sheets of
changed places) or 50 m at 10 px/m (j0, i07, new1 to new3), layers `faces,graph`. Each row: before
(left) and after (right) of one place. Blue circle: a round; orange ring: a kink; magenta ring: a
check-1 artefact (sliver, jog, fragment, bent border).

| Picture | What it shows | Does it look right? |
| --- | --- | --- |
| `kinks-fragments-gone-1.png`, `kinks-fragments-gone-2.png` | The 36 places whose kink or fragment is gone | Mostly. 15 kinks go because a new round shapes the corner (outline rounds, the radius ladder): p00, p01, p02, p03, p05, p10, p15, p16, p19, p23, p27 (new1), p29, p31, p36, p39. 20 kinks go with no new round within 3 m: the check now judges a corner by the least inner radius of the ways that form it (a road/path corner against 1 m, the owner rule) and by the turn within 1 m (a needle or a jog is no corner; they stay in the sliver and jog counts). p33 still shows a magenta artefact where its kink was. |
| `kinks-fragments-new-and-kept.png` | The 22 places with a kink or fragment after item 1 (15 at the same place as before, 7 new) | No: these are the open faults (see the report). |
| `j0-i07-new1-3.png` | j0, the i07 hairpin junction and the new1 to new3 places of step 1 | Yes for the rounds: i07's left junction has 3.0 m rounds (1.56 and 2.26 m before), new1 has its V apex rounded, new2 and new3 lose their kinks; j0 is unchanged (no kink). i07 shows two new magenta artefacts beside its left rounds. |
| `after/faces-bench.json`, `base-8d2464c-faces-bench.json` | The fixture bench of the head and of 8d2464c, run back to back (best of 3, 4 threads, 3 GB cap) | See the report. |
