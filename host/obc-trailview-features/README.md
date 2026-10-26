# TrailView feature heights

The height solvers of the `planner/trailview-features` step: the levels of rivers and lakes, and the
heights of roads, paths, tracks, bridges and tunnels. The
[spec](../../specs/trailview-features.md) says what the heights promise.

`tools/trailview/solvers.py` writes a job file, runs `trailview-heights` on it and reads the heights
back. `src/job.rs` defines both files. The step builds the binary with Cargo on first use:

```sh
cargo build --locked --release -p obc-trailview-features --bin trailview-heights
target/release/trailview-heights JOB OUT
```

| Variable | Effect |
| --- | --- |
| `OBC_TRAILVIEW_HEIGHTS` | The step runs this program instead of the Cargo build. |
| `OBC_DATA_MEMORY_BYTES` | The road jobs that run at once fit in this memory, as in the other Python steps. |

The water networks run on all cores. The road jobs run on a core each, as many as fit in the memory
that is left after the solvers read the job, by the size of the largest job.
