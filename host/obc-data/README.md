# Data publication owner

The [data contract](../../specs/obc-data.md#apply) defines publication and durable intents.

## Setup

Prepare one Linux VPS owner. Install its matching MIT `obc-data-plumbing` binary at
`/opt/obc-data/bin/obc-data-plumbing`. Install rclone. Give the owner the same `OBC_R2_*`
bucket configuration as the initiating machine. Put its environment in the owner-controlled
`/etc/obc-data/owner.env`, readable only by that account. The account must admit the fixed
system service through systemd. Create `/var/lib/obc-data/incoming` and
`/var/lib/obc-data/store`. Only the owner account may change them.

Set `OBC_COMMIT_HOST` to the configured SSH host on the laptop. Install rsync and SSH
on both machines. On that VPS, set `OBC_COMMIT_HOST=local`. Both paths run the same owner.
`OBC_DATA_STORE` selects the initiating build cache; it does not select the owner state.

For planner publication, configure `[target]` and `[publication]` in
`data/planner-runtime.toml`. Install the exact CPython and Node versions of that target,
its host libraries, systemd and Caddy on the owner. Add the configured API virtual host
to `/etc/caddy/Caddyfile`. The owner must control `/opt/obc-planner/services`,
`/etc/systemd/system`, `/etc/caddy/planner/data` and the downloads state directories.

## Operation

Prepare the selected Rust toolchain and native C compiler and linker before a run. Use the
workspace dev/release profiles and package-name overrides. Remove Cargo config build overrides
other than jobs and target directories. Remove compiler wrappers and native build flag overrides.
`RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` may set `--cfg`, lint levels and explicit codegen values for
optimization, debug information, assertions, overflow, LTO, codegen units, panic, target CPU,
target features, embedded bitcode and stripping. Use an explicit CPU instead of `native`.
Missing tools or unsupported settings fail before a build. Discovery does not install tools.
Dev/release profile environment settings and `CARGO_INCREMENTAL=0|1` enter the build identity.
Compiler children clear runtime loader search paths; workers retain them for runtime libraries.
Use compiler installations that work without custom loader paths. Preload and link-time search
overrides are refused.

`prepare`, `build` and `apply` return a run handle. On macOS the retained worker detaches
from the terminal. On Linux, set `OBC_RUN_ENV_FILE` to an absolute, private environment file.
Use standard systemd environment syntax and absolute tool paths. Prepare the locked tools,
an active systemd user manager and enabled linger for the operator. Missing setup blocks start.

Run `obc data prepare live --json`. Inspect `obc data runs RUN --follow`, then get its output
with `obc data runs RUN --result`. Review and save `.plan`. Run
`obc data apply live --plan PLAN --yes`. The initiating machine builds and verifies.
The VPS owner uploads, switches and cleans under one inherited OS lock.

`obc data runs RUN --stop` drains current preparation before stopping. It refuses after owner
handoff. Preparation is exclusive per environment on one host/store. Laptop and VPS preparation
can overlap; final publication has one fixed VPS owner. A run has no waiting queue.

After a disconnect, query the original operation on the VPS:

```sh
obc data runs RUN
obc data runs RUN --reconcile
```

Inspect `/var/lib/obc-data/store/runs/RUN.jsonl` for acknowledged writes. A pending intent
blocks every later commit. Do not delete it or retry from a later object read alone.
Reads do not change local history. Explicit reconciliation requires a verified final owner reply;
a missing record cannot disprove a delayed admission. There is no timeout takeover. Reusing a run id with another
bundle is refused. Planner apply stages and probes stored service artifacts, reloads checked
endpoint routes, then switches its pointer. Old slots and pinned routes stay through the
reader window before retirement. Unknown slot ownership blocks apply.

## Terminal controls

Run `obc data` in a terminal. Checks and edits run in the background. Press `q` to quit;
an admitted check or edit finishes before the terminal closes.

| View | Keys |
| --- | --- |
| Live region | `r` opens saved regions; `/` filters; Enter selects |
| Region editor | `n` creates an area selection; `b` creates a Box; `d` reviews deletion |
| Area selection | F5 loads the public area list; arrows and Space select; F3 shows selected areas |
| Region fields | Tab and Shift-Tab move; F2 saves the file; Esc keeps the draft |
| Sources | `f` changes scope; `/` filters; Enter shows details; `R` checks upstream |
| Source policy | `e` opens presets; `c` enters 1..65535 whole days |
| Plan | `p` opens it; Space changes source moves; `d` shows steps; `f` prepares inputs; `b` builds |
| Apply | `a` reviews the machine, live changes and removals; `y` starts the exact reviewed plan |
| Run | Enter opens progress; `R` observes; `x` stops admitted local work; `c` reconciles an owner result |
| Prepared run | `p` reviews its returned plan before build or apply |
| Error | `!` opens the full message and fix; `x` dismisses it outside an input or Run |

Region and policy writes save files for review and commit. `u` resets only the live environment;
it does not reset region files or source policies. Opening a region view does not fetch inputs.
Required Plan rows always apply. Only source moves have checkboxes; all can stay out.
An incomplete preview requires preparation and a new review. Apply does not commit saved files.
Esc hides a Run; `q` quits the viewing process. Neither stops the retained operation.
Stop drains admitted local work and prevents publication handoff. After handoff, observe or
reconcile the owner result. Progress shows acknowledged writes and unresolved owner outcomes.
After a Rust edit, new planning or work requires quitting and launching `obc data` again.
Existing Run observation, stop and reconciliation stay available.

Live apply reviews `plan.approval`, including no-change applies. `applied.approval` reports the
result. Native runtimes require Linux executables, self-contained npm and CPython
libraries. Select Python with `UV_PYTHON`; use the checked worker. Unsupported providers block
automatic approval; manual publication remains available. Containers bind exact local images.
The configured Linux owner needs a valid `/etc/machine-id` and the fixed `/var/lib/obc-data/store`.
Its current approval is owner-local. `commit-approval` reads it without changing any state.

## Portable-data API

Compare current declarations and an exact release through `local::plan`.
Name original client layers and extra paths. Review blockers before `local::adopt`.
`local::saved` reads collection roots. Adoption keeps original provenance, creates no build
receipt and starts no apps. See the [Local contract](../../specs/obc-data.md#local-portable-data).
