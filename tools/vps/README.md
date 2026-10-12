# VPS services

The follower is `obc data serve prod` as the systemd unit `obc-data-serve`. It reads the bucket
and runs the routing, search and downloads services of each planner release that prod names.
Caddy sends `/planner-api/*` of `releases.openbikecomputer.com` to it. Data never comes from this
folder: `obc data update prod` publishes it.

| File | Use |
| --- | --- |
| `deploy-services.sh` | Builds a commit on the VPS and restarts the follower |
| `obc-data-serve.service` | The unit of the follower |
| `obc-data-serve.caddy` | The Caddy route of `/planner-api/*` |

## Prepare the VPS once

Install `build-essential`, and Python and Node with the `major.minor` versions of
[`data/planner-runtime.toml`](../../data/planner-runtime.toml). Then, as root, with this folder
copied to the VPS:

```sh
useradd --system --create-home --home-dir /var/lib/obc-data --shell /usr/sbin/nologin obc-data
install -d -o obc-data -g obc-data /opt/obc-data
runuser -u obc-data -- sh -c 'curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path'
runuser -u obc-data -- sh -c 'curl -LsSf https://astral.sh/uv/install.sh | sh'
cp obc-data-serve.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable obc-data-serve
```

Deploy once, then add the route. The legacy import `/etc/caddy/planner/*.caddy` is in the site
block:

```sh
cp obc-data-serve.caddy /etc/caddy/planner/
caddy validate --config /etc/caddy/Caddyfile && systemctl reload caddy
```

## Deploy

```sh
tools/vps/deploy-services.sh --dry-run HOST   # print the commands, run none
tools/vps/deploy-services.sh HOST [COMMIT]     # COMMIT defaults to HEAD
```

`HOST` is an SSH host that logs in as root.

- Each commit builds from nothing in `/opt/obc-data/<commit>`. The first build also installs the
  toolchain of `rust-toolchain.toml`.
- `/opt/obc-data/previous` keeps the build before. To go back, deploy that commit again.
- A planner release in a format that the services do not read does not start. Deploy services
  that read it before `obc data update prod` publishes it.
- `journalctl -u obc-data-serve` shows why a release did not start.
