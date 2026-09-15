# Route encounter correction

Base: `fcad40f7`. Code: `d4236684`.

Segment-local minima created several rows for an ordinary meandering route past one place. An
encounter now means one continuous pass within the configured corridor radius. Its position is
the nearest projection on that pass. Equal distances keep the earlier segment and its signed side.
Departure from the radius followed by a return creates a new occurrence. There is no distance-based
duplicate threshold.

The query retains a record cursor and its best projection. Each step reads at most one route chunk,
in addition to its bounded map work. It does not keep another POI cache or materialize the route.
If the query anchor starts inside a pass, it resolves preceding chunks of that pass before it
applies the occurrence window. An unreadable continuation fails instead of publishing an incomplete
nearest point.

## Completed checks

- `./tools/obc test -p obc-reader -p obc-bench`
- `cargo clippy -p obc-reader -p obc-bench --all-targets -- -D warnings`
- `cargo run -p obc-bench -- --corridor`
- `cargo run -p obc-bench -- --write-golden host/obc-bench/golden.txt`
- `cargo run -p obc-bench -- --check host/obc-bench/golden.txt`
- `./tools/obc suites check`
- `cargo fmt --all`, plus `cargo fmt` in each standalone Cargo root.
- `python3 docs/build_docs.py --check-links`

The whole benchmark check passes all 17 records. All row counts and pixel hashes match the old
baseline. Thin/all returns six physical places; thin/water returns one; sparse/water returns four.
Only corridor read counters changed. The old counters predate the 64-byte POI records, which change
chunk occupancy and read-window size. The new query can also resume a record across a route seam
and read the following chunk before its nearest projection is final.

Focused tests cover meanders spanning more than 100 chunk seams, one-row forward and reverse
pages, a later return, an anchor before and after the canonical point within one pass, an equal
distance tie with opposite segment sides, and a read failure during continuation. The long-route
first-page read budget remains unchanged and passes.

One ARM reader type census reports PlaceQuery 288 bytes and EncounterScan 28 bytes. The prior
recorded PlaceQuery size is 264 bytes. Replacing the redundant validated-chunk index with a flag
limits the increase to 24 bytes per query. The two App-owned queries imply 48 additional resident
bytes; shipping CI remains the authority for App and stack totals. No limit or resource baseline
was changed here.

No image generation, UI sweep, shipping resource image build, full CI mirror, captured-input run,
or hardware test was made. Independent delta review and green integrated CI remain required.
