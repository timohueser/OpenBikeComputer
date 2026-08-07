# Pending patches to the `embedded-sdmmc` fork

The device build consumes `embedded-sdmmc` from **our own fork**, not from crates.io — see the
`[patch.crates-io]` stanza in `firmware/obc-fw-nrf54l/Cargo.toml` (and the identical one at the
workspace root, so `obc-storage`'s host tests exercise the same crate). The fork lives at
<https://github.com/timohueser/embedded-sdmmc-rs>, branch `cmd25-multiblock-write`.

This directory holds fork changes that are **written, host-tested and ready, but not yet pushed** —
because a change to a repository outside this one is the owner's to make, not an agent's.

## `embedded-sdmmc-preallocate.patch`

Adds `VolumeManager::preallocate(file, len)` plus the `FatVolume::alloc_cluster_run` allocator it
rides on, and a `tests/preallocate.rs` suite.

**What it buys.** `VolumeManager::write` extends a file one cluster at a time, the moment it runs off
the end, and every extension is four single-block device writes (`update_fat` twice, each mirrored
across both FATs). On an SD card a single-block write is a whole internal program cycle, so a
streaming upload pays those cycles *between* the multi-block bursts the CMD25 batching exists to
produce. Pre-allocating takes them out of the streaming path and makes them cheaper at the same
time: a run of consecutive free clusters is chained inside **one** FAT block and written back once,
so a FAT32 block's 128 entries turn 4 MiB of file into the four writes 32 KiB used to cost.

Measured by the patch's own test on the RAM-disk image (2 KiB clusters): writing 256 KiB leaves
**511 → 1** single-block writes in the write path, with byte-identical read-back.

### Until it is landed, the feature does not build

`sdmmc-prealloc` is **off by default and cannot be turned on against the fork as it stands**:
`cargo build --features sdmmc-prealloc` fails with `E0599: no method named `preallocate` found for
struct `VolumeManager`` at `sd::Storage::upload_reserve`. Nothing warns earlier than that — a cargo
feature has no way to assert an API on a dependency — so treat the step order below as required
rather than advisory. (The feature-on build *was* verified against a local clone carrying this
patch, so the failure is purely "the fork does not have it yet".)

### The rev it is written against

`4cada7b388f4e4bf9f8de1fcdba33f22c1245aa7` — the tip of `cmd25-multiblock-write` when the patch was
written, and the same rev `Cargo.lock` pins in both resolve roots. **CI checks out this rev, not the
branch tip**, so a commit landing on the fork cannot turn into red CI here; if the patch ever needs
rebasing, the `git am` in that step is what says so.

### How to land it

```sh
git clone --branch cmd25-multiblock-write https://github.com/timohueser/embedded-sdmmc-rs
cd embedded-sdmmc-rs
git checkout -B patched 4cada7b388f4e4bf9f8de1fcdba33f22c1245aa7
git am /path/to/OSM/firmware/patches/embedded-sdmmc-preallocate.patch
cargo test                       # 5 tests in tests/preallocate.rs + 1 unit test, plus the suite
git push HEAD:cmd25-multiblock-write
```

If the fork has moved on since that rev, rebase `patched` onto the branch tip first and re-export
the patch (`git format-patch -1 --stdout > .../embedded-sdmmc-preallocate.patch`), updating the rev
recorded above and in `.github/workflows/ci.yml`'s `FORK_BASE_REV`.

Then, back in this repo:

1. `cargo update -p embedded-sdmmc` in **both** resolve roots (the workspace root and
   `firmware/obc-fw-nrf54l`) so the two `Cargo.lock`s pick up the new fork commit.
2. Turn the `sdmmc-prealloc` feature on by default in `firmware/obc-fw-nrf54l/Cargo.toml`. The call
   site already exists — `sd::Storage::upload_reserve`, wired into the USB and BLE data planes'
   `run_upload` — and is compiled out until the feature is on, because the tree must build against
   the fork as it stands today.
3. Delete this file and the patch.

### Why a patch file rather than vendoring the fork here

Vendoring would work — the `[patch.crates-io]` stanzas would take a `path` instead of a `git` — but
it means committing ~6,000 lines of third-party code into this repo, in **two** resolve roots, and
permanently taking on the merge of every upstream `embedded-sdmmc` release by hand. That is a
supply-chain and maintenance decision, not a performance one, and the fork already exists as the
established mechanism for exactly this. A patch file keeps the change reviewable in one place and
costs one `git am`.
