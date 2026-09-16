# Integrated producer work is much cheaper than the late converter

The existing assembler can construct final direct references without copying
and rewriting the completed map. This is a real optimization opportunity.
The cost probe does not establish a complete candidate that passes the 5%
assembly regression limit. It keeps the current wire records and validator.

## Measured native work

The same release binary runs with the probe disabled and enabled. The probe
sorts the actual placement into a dense-ID-indexed scratch table, then resolves
all 1,010,635 node IDs and 2,633,471 neighbor IDs inside the existing output
walk. The reference cache is fixed at 64 KiB. The resolved values feed a
checksum; the output bytes remain v16. All six successful outputs pass normal
full verification and match the pinned 247,837,696-byte output SHA-256.

Exactly three alternating baseline/probe pairs were planned and completed.
There was one quota interruption, described below. Times are seconds.

| Pair | Baseline total | Probe total | Mapping construction | Baseline write | Probe write |
| --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 4.493578 | 4.672706 | 0.123564 | 1.006077 | 1.052972 |
| 1 | 4.888427 | 5.160615 | 0.124539 | 1.061719 | 1.137898 |
| 2 | 4.749897 | 5.164284 | 0.128020 | 1.027845 | 1.165498 |

Median total increases from 4.749897 to 5.160615 seconds, or 8.65%. Ranges
overlap, unrelated phase times vary, and quota pressure interrupted the run.
This is not a stable 8.65% architecture penalty. Mapping construction is much
more consistent: 123.6 to 128.0 milliseconds. Reference resolution is included
in write time; it has no independent timer. The per-pair write increases are
46.9, 76.2 and 137.7 milliseconds, with the same host-noise limitation.

The old late converter added 2.152428 seconds, including a second navigation
validation. The integrated probe omits that changed-format validation and
measures different work. It proves that the whole-file copy and post-write
rewrite are avoidable. It does not prove that the old complete overhead can
be replaced by the smaller mapping time alone.

Logical counts are stable across all successful samples:

| Measurement | Baseline | Probe | Extra |
| --- | ---: | ---: | ---: |
| Scratch writes, bytes | 607,409,419 | 611,451,959 | 4,042,540 |
| Scratch reads, bytes | 689,279,449 | 1,792,346,147 | 1,103,066,698 |
| Scratch read calls | 1,356,390 | 1,882,865 | 526,475 |
| Peak live scratch, bytes | 152,769,123 | 156,811,663 | 4,042,540 |

The 4-byte-per-node table is only 4.04 MB, but cached random reference resolution
adds about 1.10 GB of logical scratch reads, including the placement scan.
This read amplification makes a browser measurement important. No browser
candidate was run. Input reads, output writes and current verification reads
are identical in both modes. Overall allocator peak remains about 51.35 MB
(51,346,313 baseline; 51,346,304 probe); the nine-byte difference is not a
resource improvement. Output size and hash are unchanged by design.

## Quota interruption and evidence limits

The fourth command, pair 1 baseline, exited 1 after writing the complete output.
Its output independently matches the pinned SHA, but there is no successful
process result, so that attempt is excluded. The initial runner raised before
saving stderr. The three prior successful summaries were recovered from the
captured process stdout; their original ledgers are retained in `results.json`.

After error capture was fixed, the failed sample's first retry stopped during
scratch writing with `Disk quota exceeded (os error 122)`. That stderr is
retained in `failed-retry.log`. Only this failed sample was retried. Completed
and failed output files were moved to a task-specific cache directory to free
quota. The three remaining planned samples then completed. No successful
sample was repeated or selected out. The retained runner now saves each
successful sample and any failure before returning an error.

The input, scratch and initially written output files were on `/tmp`, a tmpfs
filesystem. These results measure native CPU and logical I/O with memory-backed
files. They do not measure physical disk, SD card or browser storage throughput.
The moved outputs were not read by later assembly samples; input and scratch
placement remained unchanged. Quota pressure and moving outputs weaken claims
about differences in elapsed time.

## Next bounded design step

The 5% assembly allowance was a chosen gate, not proof of technical optimality.
Map production runs once per assembled map; device navigation runs repeatedly.
A producer increase can be a reasonable tradeoff if device latency and energy
gains justify it. Reevaluate the allowance with device evidence rather than
silently treating this probe as a pass under the original gate.

The most promising next allocation is a 32-bit offset in 16-byte units. It
keeps planner key width unchanged and reaches 64 GiB. Actual degree counts
show only 2.08 MB raw alignment padding (2.11% of the nav section), but spatial
repacking and resulting file size remain unmeasured.

Build an actual integrated changed-format producer and validator, with dense
identity kept separate from location where useful. Measure the whole pipeline
and browser scratch traffic before a cutover decision. The capacity and RAM
tradeoffs, including cell-local directories, are in `DESIGN.md`.

No shipping source change is retained. No physical-device result, production
speedup, format adoption or proof of optimality is claimed.

## Verification

Passed:

- `CARGO_BUILD_JOBS=4 cargo build --release -p obcm-assemble --features mem-profile`
  on the replay source before measurement.
- All six successful real assemblies with full current verification, pinned
  input preflight, and independent exact output SHA-256 checks.
- `python3 -m py_compile host/obcm-assemble/dev/ng4/run.py`
  and the same command for `alignment.py`.
- `./tools/obc suites check`.
- `git apply --check host/obcm-assemble/dev/ng4/probe.patch`.
- Workspace and all three standalone-root formatting checks; `git diff --check`.

The probe patch was formatted before measurement. No shipping source remains
changed. Package unit suites and clippy were omitted for this evidence-only
change; real pinned assemblies exercise the temporary producer work. No new
unit suite, route rerun, board build, UI snapshot, browser candidate run or full
CI mirror was added. Public documentation is unchanged.
