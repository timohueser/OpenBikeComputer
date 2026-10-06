# Data publication owner

The [data contract](../../specs/obc-data.md#apply) defines publication and durable intents.

## Setup

Prepare one Linux VPS owner. Install its matching MIT `obc-data-plumbing` binary at
`/opt/obc-data/bin/obc-data-plumbing`. Install rclone. Give the owner the same `OBC_R2_*`
bucket configuration as the initiating machine. The known entry point must load these
credentials on local and SSH invocation. Create `/var/lib/obc-data/incoming` and
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

Run `obc data prepare live --json`. Review and save its `.plan`. Run
`obc data apply live --plan PLAN --yes`. The initiating machine builds and verifies.
The VPS owner uploads, switches and cleans under one inherited OS lock.

After a disconnect, query the original operation on the VPS:

```sh
/opt/obc-data/bin/obc-data-plumbing commit-status RUN
```

Inspect `/var/lib/obc-data/store/runs/RUN.jsonl` for acknowledged writes. A pending intent
blocks every later commit. Do not delete it or retry from a later object read alone.
There is no automatic reconciliation or timeout takeover. Reusing a run id with another
bundle is refused. Planner apply stages and probes stored service artifacts, reloads checked
endpoint routes, then switches its pointer. Old slots and pinned routes stay through the
reader window before retirement. Unknown slot ownership blocks apply.
