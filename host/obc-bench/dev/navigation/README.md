# Navigation cost measurement

This host harness drives the shared `NavPlanner` with its shipping workspace and
step budgets. `inputs.json` pins six current-format route cases. Coordinates are
`[longitude, latitude]` in microdegrees. Profiles are Road (0), MTB (2), and Touring
(3), with the default profile-weighted objective.

## Reproduce

From the repository root:

```sh
obc fixtures sync test
obc fixtures sync assistant
cargo build --release -p obc-bench --example nav_cost --features nav-metrics
python3 host/obc-bench/dev/navigation/run.py target/release/examples/nav_cost /tmp/ng-routes
```

The script checks map digests, runs three fresh processes per case, checks the
recorded outcome and output repeatability, and saves raw JSON and OBCR files.
`--maps DIR` uses the same package/file layout under a different root.
`--candidate` records changed input hashes for an isolated format prototype.
It does not select a production format or make candidate correctness claims.

The Grimsel route and profile pair come from `obc-route/tests/nav.rs`. Monaco uses
the registered dense ride's endpoints. The interior case adds a small offset to
the Grimsel start and checks that the nearest edge projection is interior. The
Meiringen route goes south across the network-cell latitude boundary at
46,661,632 microdegrees. The Monaco detour builds a blocked corridor from the
first dense route's emitted bytes and uses the shared detour planner.

## Measurement limits

Each plan starts with a new planner, cache, and scratch workspace. The planner's
normal reset remains enabled. Cache hits during a plan and retries are measured;
there is no artificial warm-plan run. `NavPhase` is sampled before each step.
Array entries are snap, search, and emit. Map parsing and output validation are
outside the timed interval. `NullElevation` isolates graph cost from terrain.

`ByteSource` calls and bytes count logical requested reads. All input bytes are
loaded in host RAM before timing. These are not physical SD reads or device
latency. The optional `nav-metrics` reader feature counts decoded junction records
and quadtree visits, including repeated work. It is off in shipping builds and
has no effect on their workspace layout. Count overhead is included in host time.
The after-plan interior observation does not contribute to the recorded counters.

A successful route must retain its endpoint, profile, detour, and output behavior
in candidate comparison. A matching distance alone is insufficient: compare the
emitted OBCR digests and use the existing whole route suites. Physical device
latency and board resource measurements remain unmeasured here.

## Frozen direct-lookup decision criteria

These criteria were accepted after three baseline samples and before candidate
runs. The baseline had material host timing noise. Use one fixed set of three
paired baseline/candidate runs in one idle window, with the same harness and
ordering. Do not tune parameters or repeat the experiment until it passes.

- Monaco dense median total plan time must improve at least 10%, and median search
  time at least 20%. The three-sample total-time ranges must not overlap to claim a
  measured host speedup. Overlap gives an inconclusive result.
- Other cases may regress by at most 5% of baseline median total time or 0.1 ms,
  whichever is larger. Maximum observed step time may regress by at most 10% or
  0.1 ms, whichever is larger. Keep outcomes and OBCR bytes equal.
- Workspace growth must not exceed 512 bytes. Existing board resource ceilings
  still apply. The navigation section may grow by at most 15%.
- Matched native and browser total assembly time, including producer placement,
  remap, write and full verification, may regress by at most 5%. An isolated
  converter that exceeds this limit does not establish a production solution.

Proceed with a bounded direct-lookup prototype. The dense baseline decodes 23,728
junction records and visits 26,319 quadtree nodes for 2,599 settles. Search is a
large part of total route time. These counts identify work to remove; the elapsed
criteria decide whether removing it is useful.
