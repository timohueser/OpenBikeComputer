# RA02 integration evidence

The branch includes current develop `31aec771`: weather removal, explicit route
cleanup, and the country-source fixture correction. The merge keeps authoritative
local time for place eligibility. Actual `Settings` size is 112 bytes. Whole App
and shared-vector suites, App/simulator all-target/all-feature Clippy, and the
suite registry pass after the merge. No public conceptual page changes.

CI run `34941239614`, board job `104290274164`, measured head `e44ff761`:

- App: 50,992 bytes; the exact allocation record was updated from 50,232.
- Linked resident: 306,312 bytes (300,528 B BSS and 5,784 B data).
- Scratch arena: 131,072 bytes; total uninitialized section: 132,096 bytes.
- Largest guarded poll frame: 9,784 bytes; residual main stack: 53,112 bytes.
- Largest task body: 4,048 bytes; boot-chain ceiling: 7,768 bytes.
- Recorded deep-ride high-water: 37,016 bytes; remaining margin: 16,096 bytes,
  above the unchanged 8,704-byte floor.
- App construction frame: 64 bytes, below the unchanged 4,096-byte limit.
- Flash: 1,517,884 bytes.

All device limits and recorded hardware high-water values remain unchanged.
This uses the CI head image; no local image or base rebuild was added.

The query benchmark found extra occurrences for one POI beside a meandering
route. Its expected row counts have not been changed. This behavior requires a
query fix and independent delta review. The public v15 map catalog gate also
remains pending completion and publication of the normal regional bake.

## Query correction and snapshot preparation

CI run 34944405470, board job 104300352212, measured head `2119eb49` after
the encounter fix. App is 51,040 bytes, linked resident 306,360 bytes, `.uninit`
132,096 bytes, arena 131,072 bytes, and flash 1,519,316 bytes. The largest
poll frame remains 9,784 bytes; residual stack is 53,064 bytes, with 16,048 bytes
above the recorded 37,016-byte deep-ride high-water (unchanged floor 8,704).
App initialization remains 64/4,096 bytes. Commit `15e924d4` records the actual
App allocation without changing limits.

The CI snapshot walk exposed a missing prepared detail frame. Commit `14866946`
adds the existing `f` token before activation, so the schedule read has completed.
It also pages to the seventh result on the second Resupply page for the real
detour scenario. Complete place ordering changes the old row-based destination;
the new destination gives a measured 1,078 m ordinary route and an 801 m detour
(+1 m). Named NavConfirm, RouteOverview, and DetourPreview frames passed.
The local `detour-chooser.png` records an earlier 548 m route that was too short;
it is not successful detour evidence.
The first farther-row probe omitted the page preparation and failed its expected
screen; the corrected script uses the real page boundary. No production bypass
was added. `cargo build --locked -p obc-sim`, `./tools/obc suites check`, and
`git diff --check` passed. No local full snapshot sweep or shipping image ran.
The public v15 catalog publication and independent composition delta remain open.

CI run `34945372506` passed the shipping resource gate. Its snapshot script
stopped at an old v14-only terrain staging assertion before the manifest check.
The assertion now requires the current v15 format, and the elevation plan also
prepares its detail before activation. The named `elev-nav-overview.png` and
`detour-chooser-final.png` pass through the normal simulator. CI now retains its
rendered PNG files as review artifacts so changed manifest entries can be
inspected without another local sweep. No expectation is regenerated in CI.

The next CI walk reached Up ahead. Complete encounter ordering moved its old
POI-detail cursor onto an authored waypoint, where Select is correctly inert.
The recipe now selects the actual map POI with eight steps. Its named detail
frame passed. This changes only the fixture journey; waypoint behavior is intact.
