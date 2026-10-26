# VPS services

The follower is `obc data serve prod` as the systemd unit `obc-data-serve`. It reads the bucket
and runs the routing, search and downloads services of each planner release that prod names.
Caddy sends `/planner-api/*` of `releases.openbikecomputer.com` to it. Data never comes from this
folder: `obc data update prod` publishes it.

| File | Use |
| --- | --- |
| `deploy-services.sh` | Builds a commit on the VPS and restarts the follower; `obc data deploy services` runs it |
| `obc-data-serve.service` | The unit of the follower |
| `obc-data-serve.caddy` | The Caddy route of `/planner-api/*` |

## Prepare the VPS once

`obc data deploy`, and Services on the prod screen of `obc data ui`, check the one-time setup over
SSH and print each missing step with its command: the user `obc-data`, `/opt/obc-data`, Rust and
uv of that user, `build-essential`, Python and Node of
[`data/planner-runtime.toml`](../../data/planner-runtime.toml), the unit, and after the first
deploy the Caddy route. The site block imports `/etc/caddy/planner/*.caddy`.

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
- `/opt/obc-data/previous` keeps the build before. To go back, deploy that commit again.
- A planner release in a format that the services do not read does not start. Deploy services
  that read it before `obc data update prod` publishes it: the update waits up to 24 h for the
  follower, and then leaves prod as it was.
- The follower keeps the objects of the releases that prod names, and nothing else, in
  `/var/lib/obc-data/store/follower/prod/`.
- `journalctl -u obc-data-serve` shows why a release did not start.
- `/planner-api/version` gives the commit of the build and the layer formats that its services read.
