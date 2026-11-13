# Second cloud session of epic #3227

This folder is an archive of the second cloud session. Never merge this branch. The state of the
work is in the issues: epic #3227 (the plan with its ticks and the comment "State at the end of
the second build session"), each sub-issue (`Status:` line, "Built differently" or "Progress"),
and the pull requests. `../README.md` has the notes of the first session, which still hold where
this file says nothing else.

| Path | What |
| --- | --- |
| `container.md` | The rules that each implementer of this session read first |
| `bake-README.md` | The shared bake data of this container: the inputs, the new baseline and its hashes, the gate command |
| `setup.sh`, `download.sh`, `baseline.sh` | The scripts that made the container: the tools, the Switzerland inputs, the baseline bake and its packages |
| `results/` | The `result.json` of the baseline bake and of the layout B bake of #3310 |

## What differs from the first session

- `download.osgeo.org` is not reachable from the cloud container, so `tools/install-geos.sh` cannot
  download GEOS. `setup.sh` builds GEOS 3.14.1 from the release tarball of `github.com/libgeos/geos`
  and then lets `tools/install-geos.sh` write the environment file.
- A new container has a new OSM extract, so the hashes of `../bake-README.md` do not compare. The
  baseline of this session is `routing-overlay` at `67320e4` on the extract with the replication
  timestamp 2026-11-13T01:20:21Z; `bake-README.md` has its hashes.
- The container restarted once and killed the agents and a bake without a notice. Each
  implementer pushed after each step, and a scheduled check-in of the orchestrator looked for lost
  work. Take the build lock for each phase of a long script, not for the whole script.
- With `uv`, Python 3.14 is present, so the Python steps of `planner_build.rs` run.
- A reviewer on the strongest model reviewed #3305; the other reviewers and the implementers ran
  on the model that `cairn model` names.
