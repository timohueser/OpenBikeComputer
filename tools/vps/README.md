# VPS services

The follower is `obc data serve prod` as the systemd unit `obc-data-serve`. It reads the bucket
and runs the routing, search and downloads services of each planner release that prod names.
Caddy sends `/planner-api/*` of `releases.openbikecomputer.com` to it. Data never comes from this
folder: `obc data update prod` publishes it.

## Setup

`obc data deploy`, and Services on the prod screen of `obc data ui`, check the one-time setup over
SSH and print each missing step with its command: the user `obc-data`, `/opt/obc-data`, Rust,
uv and the uv Python of [`.python-version`](../../.python-version) of that user, `build-essential`,
Node of [`data/planner-runtime.toml`](../../data/planner-runtime.toml), the unit, the limit of
memory maps, and after the first deploy the Caddy route. The site block imports `/etc/caddy/planner/*.caddy`.

The machine of the page logs in as root with an SSH key, without a prompt. The screen shows the
line of its public key to add on the VPS, and the fingerprint of the host key to compare with the
console of the hosting provider before it trusts the key.

## Deploy

Deploy from the prod screen of `obc data ui`, Services, or with the same commands in a terminal:

```sh
obc data deploy                                  # the commit that runs, and what a deploy lacks
obc data deploy services --dry-run               # the commit of this checkout, and the commands
obc data deploy services --expect COMMIT         # deploy it in a run
```

`OBC_VPS_SSH` is an SSH host that logs in as root. The screen sets it, or
`obc data deploy credential services` from standard input. The deploy runs:

```sh
tools/vps/deploy-services.sh --dry-run HOST   # print the commands, run none
tools/vps/deploy-services.sh HOST [COMMIT]     # COMMIT defaults to HEAD
```

- Each commit builds from nothing in `/opt/obc-data/<commit>`. The first build also installs the
  toolchain of `rust-toolchain.toml`.
- The deploy installs the unit of its commit and `obc-data.sysctl` as
  `/etc/sysctl.d/90-obc-data.conf`, and applies the limit when the kernel has a lower one. Then it
  switches to the new build and restarts the follower. A failed step before the switch leaves the
  build that runs.
- The routing service maps each file of its routing cells. When `vm.max_map_count` is lower than
  the files of an install need, the routing service does not start, and `journalctl -u
  obc-data-serve` shows its message. Install the sysctl file and run `sysctl --system`: the
  follower starts the service again at its next retry, with no download.
- `/opt/obc-data/previous` keeps the build before. To go back, deploy that commit again.
- A planner release in a format that the services do not read does not start. Deploy services
  that read it before `obc data update prod` publishes it: the update waits up to 24 h for the
  follower, and then leaves prod as it was.
- The follower keeps the objects of the releases that prod names, and nothing else, in
  `/var/lib/obc-data/store/follower/prod/`.
- The follower keeps the installed files in `/var/lib/obc-data/store/serve/prod/`. A restart
  installs nothing again. A rollback after a failed update downloads nothing while the disk kept
  its files.
- `journalctl -u obc-data-serve` shows why a release did not start.
- After a disk fault, read the installed routing cells of each release again. The command fails
  and names each file whose SHA-256 differs from the directory of its package:

  ```sh
  /opt/obc-data/current/target/release/planner-service ROUTE_CELLS --scrub
  ```

  `ROUTE_CELLS` is the folder of the installed routing cells of the release.
- `/planner-api/version` gives the commit of the build, the layer formats that its services read
  and the format of the release manifest that it reads.

## Reference

| File | Use |
| --- | --- |
| `deploy-services.sh` | Builds a commit on the VPS and restarts the follower; `obc data deploy services` runs it |
| `obc-data-serve.service` | The unit of the follower |
| `obc-data.sysctl` | `vm.max_map_count` for the memory maps of the routing service |
| `obc-data-serve.caddy` | The Caddy route of `/planner-api/*` |
