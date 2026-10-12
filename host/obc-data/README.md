# Data operations

Run `obc data` from the repository root. Without a command it shows each target: what it serves,
how its layers stand, and the command to run next. Use `--json` for structured output.
The [data guide](../../docs/content/software/data.md) explains the model.
The [data contract](../../specs/obc-data.md) defines bytes and admission checks.

## Setup

Prepare Rust, Python with `uv`, GEOS and Osmium before an update; `obc data` does not install them.
`obc data setup` shows what this machine can do and the tools and credentials that it has.
A changed producer does not rebuild its layer. Raise the `revision` of the layer in the change
that changes its output; a reader that needs new data raises its `format`.

`data/targets.toml` names the targets. `local` is a folder beside the store; `obc data update`
writes only a folder target. The settings of a target (region, optional layers, refresh
policies) are in the release that it serves, so they need no commit.

| Configuration or credential | Purpose |
| --- | --- |
| `data/targets.toml` | Targets and where they are |
| `data/machines.toml` | The header colour and the SSH host of each machine for the page |
| `data/regions/` | Region presets |
| `data/sources.toml` | Source declarations and their refresh policies |
| `data/planner.toml` | Planner producer options |
| `data/planner-runtime.toml` | Runtime target and publication origins |
| `OBC_DATA_STORE` | Build store |
| `OBC_DATA_MACHINE` | The machine name in a release; the short host name without it |
| `OBC_R2_BUCKET`, `OBC_R2_ACCESS_KEY_ID`, `OBC_R2_SECRET_ACCESS_KEY` | Production bucket and key, for `obc data r2` |
| `OBC_R2_ACCOUNT_ID` or `OBC_R2_ENDPOINT` | Cloudflare account or explicit endpoint |
| `OBC_FIXTURE_R2_*` | Separate fixture bucket and credentials, with the same suffixes as production |
| `UV_PYTHON` | Selected prepared Python interpreter |
| `~/.cdsapirc` | CDS credential for a selected climate fetch |
| `~/.config/openbikecomputer/cdse-s3.env` | CDSE S3 credentials for a selected snow fetch |

Keep secrets in the private host environment or ignored `tools/obc.local`, never tracked config.

## Daily commands

| Command | Action |
| --- | --- |
| `obc data` | Show each target and what needs attention |
| `obc data update local --region ID` | Name the region of a target, bake and switch |
| `obc data update local` | Bake what is stale, keep the rest, switch, in a run that closing the terminal does not stop |
| `obc data update local --dry-run` | Review the update: what bakes and why, the cost, and its id |
| `obc data update local --expect ID` | Start the update only when its review still has this id |
| `obc data update local --add sun` | Switch an optional layer on; `--remove` switches it off |
| `obc data update local --refresh wikidata=30` | Set a refresh policy for the target |
| `obc data update local --only LAYER` | Bake a layer again after a code change with the same version |
| `obc data rollback local` | Serve the rollback release again; a second rollback undoes it |
| `obc data source refresh natural-earth` | Move a `digest` source to upstream's newest version |
| `obc data serve` | Serve `local` to the apps until Ctrl-C |
| `obc data runs RUN --follow` | Observe progress; Ctrl-C ends only the following |
| `obc data runs RUN --stop` | Stop a run: it frees the lock and keeps what it baked |
| `obc data unlock local --from MACHINE` | Free the lock of a run that stopped on that machine |
| `obc data clean` | Review unused store bytes before `--apply` |
| `obc data ui` | Serve the page of this machine at `http://127.0.0.1:8700/` |
| `obc data open MACHINE` | Open the page of another machine through an SSH tunnel |

An update runs in a process of its own: Ctrl-C ends only the following of its log, and
`obc data runs RUN --stop` stops it. An update that stops keeps what it baked: run it again. The
previous release stays as the rollback of the target. A target takes one update or rollback at a
time: the run holds `lock.json` of the target, and another run refuses to start. A run that fails
or stops frees the lock. A crash or an out-of-memory kill keeps it:
`obc data unlock TARGET --from MACHINE` frees it after 10 minutes without a heartbeat.

The [page README](../../builder/web/src/components/data/README.md) tells how to run and develop
the page.

## Serve

`obc data serve` serves `local` on port 8710 of every IPv4 address of the machine. It prints the
command that opens each app on `local`: the web planner, the map builder, the desktop app, the
simulator and the Debug iOS app. It runs the routing, search and downloads services of each
planner release and the tile server of `planner/tiles`. At start it builds `planner-service` with
cargo. Prepare Node and the other services once:

```sh
python3 planner/search/setup.py
npm ci --prefix planner/tiles
npm ci --prefix builder/web
npm run build:wasm --prefix builder/web
```

Leave it running while `obc data update local` runs in another terminal. Clients get the new
release when its services answer. A failed release leaves clients on the current release; the
services write the cause to the terminal. A service that stops later does not start again: stop
`obc data serve` with Ctrl-C and start it again.
Serve reads the formats of the services once, at start: start it again after a change of the
format that a service reads.

- Every machine on the local network reaches every file of the target, also `release.json`, and
  the three services. Any web page can read them (CORS `*`).
- The download bundles name the network address that the machine has at start. After a change
  of network, start `obc data serve` again.
- The services read their files from `<store>/serve/local/`. Serve removes it when it starts and
  when it stops; `obc data clean` does not touch it.
