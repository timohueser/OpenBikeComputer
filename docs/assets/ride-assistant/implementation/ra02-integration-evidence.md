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
