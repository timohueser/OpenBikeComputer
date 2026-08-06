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

### How to land it

```sh
git clone --branch cmd25-multiblock-write https://github.com/timohueser/embedded-sdmmc-rs
cd embedded-sdmmc-rs
git am /path/to/OSM/firmware/patches/embedded-sdmmc-preallocate.patch
cargo test                       # 3 new tests in tests/preallocate.rs, plus the existing suite
git push
```

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
