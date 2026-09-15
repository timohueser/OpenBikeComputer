# Navigation optimization follow-up

## Browser cache experiment

The retained baseline reads 116,438,699,584 logical input bytes for a
247,837,696-byte output. The current browser cache fetches 64 KiB on a miss,
with 16 shared slots. Its claim of sequential reads needs checking against
actual assembly traffic.

Compare the existing `readBlockBytes` option at 65,536 and 4,096 bytes. This
changes both the input cache and output verification cache. It uses the same
shipping WASM, pinned inputs, sort budget, persistent Chromium worker, full
validation, and output digest read-back. No production defaults change.

Before measurement, fix three pairs in this order: baseline/candidate,
candidate/baseline, baseline/candidate. Do not run other project benchmarks or
builds during the samples. Retain all samples; do not remove slow ones.

A useful result requires at least 10% lower median engine total, disjoint total
time ranges, identical verified output hashes, and no increased WASM capacity.
This tests one spilling workload. It cannot select a universal cache default or
prove physical disk savings. Input staging and independent output read-back
remain outside the engine timing, as in the baseline harness.

### Results

| Engine phase | 64 KiB median | 4 KiB median | Change |
| --- | ---: | ---: | ---: |
| Total | 19.278 s | 10.853 s | -43.7% |
| Navigation | 9.600 s | 4.799 s | -50.0% |
| Write | 7.944 s | 4.399 s | -44.6% |
| Full verification | 1.399 s | 1.606 s | +14.8% |

The three total ranges are 17.541–19.617 s and 9.656–11.455 s; they do not
intersect. Both groups became faster later in the window, but every pair showed
the same direction and all raw samples remain. These are host measurements for
this fixed workload, not a universal percentage.

Logical input read bytes fell from 116,438,699,584 to 9,066,983,061 (-92.2%).
Input calls rose from 1,774,769 to 2,172,442 (+22.4%). Verification reads fell
from 2,203,309,628 to 731,796,476 bytes, but calls rose from 33,429 to 175,496;
verification time increased. Scratch traffic and output bytes were unchanged.
WASM capacity fell from 70,123,520 to 69,140,480 bytes. This is linear-memory
capacity, not browser process memory or a measured physical storage transfer.

All six independent output hashes equal
`feb9775a380b5aea0736b01c14a7af7ea0d3cd025a9e8b74c0d629a164bf5e72`.
Every output passed the existing complete-map validator. The predeclared
workload criterion passed.

The same shipping WASM from NG1 was used in every sample, SHA-256
`af970b4d3b579d1d7e5083e0acb2322db44c824be884637c2e5f39fa60c066a7`, built at
`55be89b653f2094cc67c192e3fd2fa31673a8cd8`. The raw `source_commit` identifies
the follow-up checkout (`3d4bae34`); the browser/assembler shipping code is
unchanged between those sources. Only the retained measurement harness and
option vary. Its measured SHA-256 is
`837a2803a9d63b1b5b76ba984e63be4df5149c0402b89a72558b11ea2fff3941`.
Each raw JSON also records Chromium, OS, manifest digest and timestamp.
The persistent browser profile was created under `/tmp`, which this host mounts
as `tmpfs`. OPFS uses real filesystem handles, but their host backing is RAM.
These samples do not establish performance on an SSD or SD card.

### Interpretation and next implementation

The current source comments assume each input access stream is sequential and
that 64 KiB fills have negligible amplification. The measured traffic contradicts
that assumption for this workload. The cache has sixteen shared slots; the
assembler reads edge records in final graph order. Input prefetch size needs a
measured choice, independent of any graph format change.

Use a smaller input block as the next bounded production candidate. Keep output
verification caching separate: the same option currently changes both caches,
and smaller verification blocks cost more time here. Validate the selected
policy on the existing browser correctness suite and a workload with sequential
source access before changing the default. This report selects that work; it
does not change the default or claim the larger browser-memory acceptance gate.

### Reproduce

Follow `../navigation/README.md` to install the browser dependencies and build
the shipping WASM. Fetch the pinned objects with `../navigation/fetch.py`. Run
`../navigation/browser.mjs INPUT OUTPUT READ_BLOCK_BYTES` for the fixed sequence
`65536, 4096, 4096, 65536, 65536, 4096`. Each run uses a new persistent profile,
verifies the input digests, and independently reads back the output hash. The
six numbered JSON files are the complete run results.
