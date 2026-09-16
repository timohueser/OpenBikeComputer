# Flat-store line budget

Run the absolute count from the repository root:

```sh
python3 tools/loc_ledger.py --storage-total --check-budget
python3 tools/loc_ledger.py --storage-total --head COMMIT
```

`obc loc-ledger` accepts the same arguments. Python's standard library and Git are the only
requirements. The command reads the selected commit, prints its full ID, and does not read source
from the working tree. Commit source changes before you measure them. No base build or device is
required. `--check-budget` returns 1 above 6,000 raw production lines and 0 at or below that limit.
Without it, a valid report returns 0 even when over budget. Missing revisions or an empty scope
fail. There is no new CI gate; this command supplies a reproducible acceptance check.

## Fixed scope and method

The scope is every tracked `.rs` file recursively under `firmware/obc-storage/src/flat/`, pinned
by `STORAGE_SERIES_PATHS` in [loc_ledger.py](loc_ledger.py). New Rust files in this directory enter
the count automatically, even if they are not yet declared as modules. This includes allocation,
catalogs, journals, geometry, leases, metadata, route cleanup and the storage wire binder.

The two columns use the existing counter:

- **Raw production:** each source line, including comments and blank lines, after the exclusions
  below. A final line without a newline counts once. A terminal newline adds no extra line.
  **The maximum is 6,000 on this basis.**
- **Production code:** each line that has a token after the lexer removes comments and literal
  contents. This is a structural count. Lines wholly inside multiline string literals have no
  structural token and do not count. This supplemental column does not change the raw ceiling.

The report lists every file, including wholly excluded files. Its raw production and excluded
raw columns sum to all Rust source lines in the fixed scope. Deleted files provide no credit.

### Test and harness exclusions

The absolute report excludes:

1. Files in `tests/`, `benches/`, `fixtures/`, `testdata/`, `test-data/`, `golden/` or `vectors/`
   directories; bench-named files in `bin/`; and `*_test.rs`, `*_tests.rs` or `test_*.rs` files.
2. Items with an exact `#[cfg(test)]` gate, including the gate and its complete item. A file with
   `#![cfg(test)]` is wholly excluded. Conventional external module declarations pass this rule
   to their child modules. Comments and blank lines outside the gated item remain production.
3. The named `model.rs` and `sim.rs` modules. These are the independent model, sparse test disk
   and fault-injection backend. Their `std` feature exposes them to external test crates.
   The explicit names live in `STORAGE_HARNESS_FILES` and require review if their role changes.

All other conditions remain production in the absolute report. In particular,
`cfg(any(test, feature = "std"))` does not prove that an item is test-only. Its module declarations,
store observations and helpers remain counted unless they are inside an explicitly excluded file.

### Scanner and boundary limits

The scanner removes comments and literal contents, then balances braces to find an item's end.
It handles nested comments, raw strings, lifetime ticks and array semicolons. It does not compile
Rust, expand macros, follow `include!` or `#[path]`, or evaluate general conditional expressions.
Review new syntax or module indirection before using its result as acceptance evidence.

This boundary excludes platform adapters and product execution in the board and host crates,
physical SD drivers, remaining FAT/update users, the v4 engine in `obc-link`, and format codecs.
It is the existing flat-layer budget boundary, not a count of all storage-related runtime code.
FAT removal, adapter convergence, update install/rollback and physical fault acceptance remain
separate requirements. Retired OBC2/v3 deletions cannot offset this count.

## Recorded reconciliation

Source commit: `773758a82af59c728ff51b4b699b5a07d1e47609`.
Reproduce with the absolute command and `--head` set to this commit. These are source counts,
not device measurements. All module paths below are relative to the fixed scope above.

| Module | Production raw | Production code | Excluded test raw |
| --- | ---: | ---: | ---: |
| `bitmap.rs` | 113 | 71 | 82 |
| `catalog.rs` | 302 | 240 | 201 |
| `cost.rs` | 0 | 0 | 342 |
| `crash.rs` | 0 | 0 | 2,599 |
| `device.rs` | 28 | 7 | 0 |
| `error.rs` | 116 | 54 | 0 |
| `fence.rs` | 0 | 0 | 200 |
| `fuzz.rs` | 0 | 0 | 345 |
| `granularity.rs` | 0 | 0 | 377 |
| `journal.rs` | 193 | 142 | 136 |
| `layout.rs` | 364 | 193 | 241 |
| `map_read.rs` | 0 | 0 | 245 |
| `metadata.rs` | 681 | 619 | 2 |
| `metadata/tests.rs` | 0 | 0 | 556 |
| `mod.rs` | 37 | 27 | 18 |
| `model.rs` | 0 | 0 | 209 |
| `raw.rs` | 53 | 34 | 24 |
| `read_cost.rs` | 0 | 0 | 188 |
| `route_cleanup.rs` | 35 | 32 | 56 |
| `sealed.rs` | 0 | 0 | 148 |
| `seam.rs` | 318 | 183 | 52 |
| `sim.rs` | 0 | 0 | 656 |
| `source.rs` | 212 | 117 | 411 |
| `store.rs` | 2,421 | 1,609 | 10 |
| `superblock.rs` | 96 | 57 | 76 |
| `vectors.rs` | 0 | 0 | 378 |
| `wire.rs` | 254 | 204 | 62 |
| **Total** | **5,223** | **3,589** | **7,614** |

The raw reconciliation is **5,223 + 7,614 = 12,837** source lines. The flat layer is **777 raw
production lines below** the 6,000 maximum at this commit. This does not close the wider storage
acceptance gate.

The report counts 26 mixed test/std lines as production: 4 in `mod.rs`, 6 in `layout.rs`, 4 in
`source.rs` and 12 in `store.rs`. A `cfg` that merely mentions `test` does not prove an item is
test-only, so they stay in the total.
