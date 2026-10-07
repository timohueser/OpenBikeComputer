# Linux bake setup

Use the [data contract](../../../../specs/obc-data.md#automatic-work).

Prepare a Linux operator with systemd user services, enabled linger and delegated cgroup-v2 CPU
and memory controllers. Prepare the locked offline Cargo dependencies and the selected native tools.
Create the configured build-store directory.
Update `/opt/obc-data/bin/obc-data-plumbing` so it supports `bake-preflight`.

Set `OBC_RUN_ENV_FILE` to an absolute operator-owned file with mode `0600`.
Use standard systemd environment syntax. Export the same setup settings when running manual commands.
Keep credentials in this private file. Use absolute tool paths and a PATH with the prepared Cargo
and runtime tools. Select Python with `UV_PYTHON`.

| Setting | Value |
| --- | --- |
| `OBC_DATA_STORE` | Absolute build store |
| `OBC_BAKE_CPU_PERCENT` | Owner-chosen positive CPU percentage for concurrent bakes |
| `OBC_BAKE_MEMORY_MIB` | Owner-chosen positive total bake memory limit |
| `OBC_BAKE_MIN_FREE_MIB` | Owner-chosen positive disk reserve |
| `OBC_BAKE_ALERT_UNIT` | Installed operator notification service, outside `obc-data-*` |

For an optimized host worker, use the existing `CARGO_PROFILE_DEV_OPT_LEVEL=3` setting.
Profile settings enter the checked execution identity. Run `obc data schedule live --setup-budget` to install and verify the slice without a timer.
Then review a complete manual Live apply on this host before automatic work. Set `OBC_PLANNER_RUNTIME_BUILDER=native` for new runtime builds on the
budgeted Linux host. Docker daemon builds do not inherit the bake slice; verified cached artifacts
remain usable. Laptop container publication stays supported. Keep the installed commit owner and
serving services outside the bake slice.

Enable an explicit calendar and time zone:

```sh
obc data schedule live --calendar weekly --time-zone Europe/Berlin
obc data schedule live
obc data schedule live --disable
```

Inspect `enabled`, `active`, `runnable`, the blocked reason and the last run. Changes to checkout code
use the fresh launcher at the next occurrence. Changes to command context or host setup need a reviewed
schedule save. Disable leaves active work running through verification; it prevents a new handoff.

Check representative host throughput, controller limits, disk reserve and real notification delivery
before unattended use. Disk checks are preflights. Use an operator-managed volume or quota for hard disk
isolation. Unit fixtures alone do not prove host readiness.
