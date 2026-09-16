# Pinned navigation assembly measurement

This is a bounded measurement input and two small runners. It reuses the native
assembler, `mem-profile`, the shipping browser worker, and its OPFS ledger.
It is not the country-scale acceptance campaign.

## Inputs and reproduction

`inputs.json` retains 41 content-addressed catalog objects: one complete coarse
square, four mid cells, 16 fine cells, 16 network cells, and four terrain cells.
All map inputs use OBCM v16. The 248,096,256 input bytes come from the published
Baden-Württemberg catalog snapshot identified in that file. It contains the
schema, skin, source URLs, byte sizes, digests, source snapshot dates, and build
timestamps. The catalog does not expose a producer Git commit; that remains
unknown. The content digests identify the exact measured input bytes.

From the repository root:

```sh
python3 host/obcm-assemble/dev/navigation/fetch.py /tmp/ng-inputs
cargo build --release -p obcm-assemble --features mem-profile
python3 host/obcm-assemble/dev/navigation/native.py target/release/obcm-assemble /tmp/ng-inputs /tmp/ng-native
```

The native runner uses a 16 MiB merge budget, includes terrain, accepts marked
partial inputs, refuses holes, and leaves full output verification enabled. It
checks local sidecar identities, order, schema, skin, terrain lattice, and payload
digests before timing. It refuses drift without downloading inputs. Add
`--check-inputs` to run only this preflight. It runs exactly three assemblies
otherwise. Each output is read back independently for a
SHA-256 check. Native JSON and stderr are retained together with binary, source,
platform, and input-manifest identities. Keep the machine idle for elapsed-time
comparisons. Do not run compilers or another assembly at the same time.

Build the worker and install the existing browser-test dependency:

```sh
(cd builder/app && npm ci && npm run build:wasm:assemble)
(cd apps/obc-web-demo/tests/browser && npm ci)
node host/obcm-assemble/dev/navigation/browser.mjs /tmp/ng-inputs /tmp/ng-browser.json
```

The runner defaults to `/usr/bin/google-chrome`; set `CHROME_BIN` for another
Chromium executable. It starts a local Vite server and a fresh persistent browser
context. OPFS inputs, output and scratch use that context; no user browser profile
is opened. It verifies input hashes before staging and reads back output in bounded
chunks for an independent digest. Input staging, WASM initialization and output
readback are outside the request timer. Use `summary.phases_us.total` as the common
native/browser engine timing boundary; request time also includes worker setup and
cleanup. The single retained browser baseline is an observation, not a repeatability
claim. Use three paired samples if a candidate reaches the comparative timing gate.

## What the measurements mean

Native `mem-profile` measures allocator-owned live bytes and the maximum observed
live allocation. It is not process RSS. Its IO ledger counts calls and requested
bytes at the native `ByteSource`, output store, and scratch-store seams. These
include cached operating-system IO and do not measure physical disk traffic.
Scratch peak is the maximum live logical scratch-file size across all phases.

The native navigation subphase labels report elapsed time between boundaries:
collection, deduplication, pruning, renumbering, endpoint joins, edge placement
and anchor generation, snap index, and adjacency with node index. The latter
passes include their external sorts. Output writing and full verification remain
separate top-level phases. The optional clock hook is inert for normal callers.

The browser runner uses the actual worker and sync OPFS in a persistent,
non-incognito context. A measurement-only Vite transform counts logical live scratch
bytes at append/remove without changing storage policy. A wrapper captures the
actual instantiated WASM memory before worker initialization. Its final WASM linear
memory capacity, the estimator result, and native allocator peak are different
quantities. None bounds browser process memory. Browser read counts refer to its
OPFS window cache and can differ sharply from native byte-source requests.
Independent output readback is outside reported assembly time.

## Native baseline

The three runs in `native.json` used source `55be89b6` and the executable digest
recorded in that file. Total times were 4.857, 4.865, and 4.910 seconds: a 1.1%
range relative to the median. All outputs were 247,837,696 bytes with SHA-256
`feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72`.
Full verification stayed enabled. The output has 1,010,635 nodes and 1,317,371
edges, with no dropped nodes or truncated adjacency entries.

| Native phase | Median milliseconds | Share of median total |
| --- | ---: | ---: |
| Navigation | 2,433 | 50.0% |
| Write | 1,030 | 21.2% |
| Verify | 1,378 | 28.3% |
| Collection, inside navigation | 501 | 10.3% |
| Deduplication | 66 | 1.4% |
| Pruning | 70 | 1.4% |
| Renumbering | 181 | 3.7% |
| Endpoint joins | 443 | 9.1% |
| Edge placement and anchors | 768 | 15.8% |
| Snap index | 7 | 0.2% |
| Adjacency and node index | 399 | 8.2% |

Medians of individual phases do not sum to the median total. Peak allocator-owned
memory was 51,346,322 bytes. Peak live scratch was 152,769,123 bytes. Scratch writes
were 607,409,419 bytes in 244 calls, with 689,279,449 read bytes in 1,356,390 calls.
This is a spilled navigation workload, not an in-memory small graph.

Input reads totalled 341,256,925 bytes in 2,758,507 calls. Verification reads totalled
1,103,026,810 bytes in 1,971,277 calls. Output writes totalled 247,837,696 bytes in
239 calls. The logs retain the exact counts and all three timing samples.

## Frozen cell-preservation criterion

The baseline supports a bounded prototype: renumbering and endpoint joins alone
account for 12.8% of total native time. A design can also reduce layout and index
work for unchanged cells. This is a potential saving, not a measured candidate.

Require at least a 10% reduction in median total assembly time on both native and
actual browser runs, with full output verification. Use three fixed samples per
implementation and require non-overlapping total-time ranges for a speedup claim;
overlap is inconclusive. The 10% target exceeds the native baseline's 1.1% range.
Allow at most 5% growth in peak memory and output size. Keep route correctness
and device resource ceilings. These thresholds were sent to the candidate owner
before any cell-preservation candidate run. The browser baseline confirms that
navigation is material: 8.736 seconds, or 44.2% of total engine time. Proceed with
a bounded prototype. Neither baseline selects a new production graph format.

Collection, pruning, output writing, and verification cannot all disappear in a
cell-preserving design. An upper bound that removes all navigation time is only
an early rejection check. A useful prototype must account for seam aliases and
changed cells; operation-count reductions alone do not establish a time saving.

## Recorded browser baseline

`browser-baseline.json` retains the run on Chromium 153.0.8010.36 and source
`55be89b653f2094cc67c192e3fd2fa31673a8cd8`. The recorded WASM digest identifies
the rebuilt executable. Full verification passed, with 1,010,635 graph nodes.
Native and browser output are both 247,837,696 bytes with SHA-256
`feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72`.

| Observation | Browser |
| --- | ---: |
| Engine total | 19.784 s |
| Navigation | 8.736 s (44.2%) |
| Write | 9.196 s |
| Verification | 1.797 s |
| Request through worker completion | 19.941 s |
| WASM linear-memory capacity | 70,123,520 B |
| Estimated peak working memory | 149,757,133 B |
| Peak logical live scratch | 152,769,123 B |
| Input OPFS calls / bytes | 1,774,769 / 116,438,699,584 |
| Scratch writes / bytes | 244 / 607,409,419 |

The large input-read byte count is logical OPFS traffic through the existing
64 KiB window cache, not physical storage traffic. It includes input reads during
output writing. This is relevant to #1162; the graph rewrite cannot be credited
with removing all browser IO. Keep the current cache configuration for comparison.

No physical device latency, browser-process RSS, or JavaScript heap ceiling was
measured. An initial incognito diagnostic was stopped and excluded because its
storage backing was not representative. The retained result uses a fresh persistent
profile with actual OPFS files. This evidence does not close #1162, #1503 or #1420.
