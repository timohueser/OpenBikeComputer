# Handover of epic #3227 (routing: hierarchical search and disk storage)

This branch is an archive of one cloud session. Never merge it. Delete it when the epic is done.
The state of the work is in the issues: epic #3227 (the plan with its ticks), each sub-issue
(`Status:` line and a "Built differently" section), and the pull requests.

## What is here

| Path | What |
| --- | --- |
| `container.md` | The rules that each implementer and reviewer of the session read first: the tools, one build at a time, the checks that CI runs, git and pull request format |
| `bake-README.md` | The shared bake data of the container: inputs, baselines with their hashes, the command of each gate, the runs of each sub-issue |
| `tools/bake_layers.py`, `tools/bake_routing.py` | The scripts that run the routing steps outside `obc data` and measure them |
| `tools/region_gate.rs`, `tools/tables_gate.rs` | The programs that compared the new data layer with format 9 (#3302) and each cost table entry with a plain search (#3304); they are not committed tests |
| `results/` | The `result.json` of each measured bake |
| `3381-results.log` | The partition timings of #3381 |

## A new container

The container of the session is gone with the session. A new one needs:

```sh
cargo install just --locked
bash tools/install-geos.sh          # GEOS 3.14.1 into ~/geos-install
apt-get update && apt-get install -y osmium-tool
cargo install cargo-nextest --locked
cargo install cargo-deny@0.20.2 --locked
cargo install cargo-about@0.9.1 --locked --features cli
rustup update stable                # CI tracks the newest stable
```

`obc` is `./tools/obc` in a checkout. `obc-data-steps` tests need `npm ci` in `builder/web`.
10 tests of `obc-data` fail in a cloud container on every branch: 8 need `rclone`, 1 needs ssh
host keys, 1 fails because the tests run as root.

The bake inputs are gone too. `bake-README.md` gives the download and clip commands. A new
download of the OSM extract has a newer replication timestamp, so its packages have other hashes
than the ones recorded here: make a new baseline from the head of `routing-overlay` first, and
compare each change with that baseline.

The disk of a cloud container is about 30 GB. A full Switzerland bake with its layers needs about
10 GB while it runs; Germany does not fit. The owner runs the Germany bakes of #3310 on his PC, at
the end.

## How the session worked

One implementer (Opus) for each sub-issue in its own worktree, one fresh reviewer that follows
`.claude/skills/cairn/reference/review.md`, a delta review after fixes, the merge with a merge
commit when the review is clean and CI is green, then `Status: done`, a "Built differently"
section, the issue closed by hand (a merge into `routing-overlay` does not close it) and its tick
in #3227. After two failed review rounds on one pull request: stop and report to the owner.
