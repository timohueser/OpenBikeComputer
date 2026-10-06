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
