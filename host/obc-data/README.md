# Data operations

Run `obc data` from the repository root. Without a command it shows each target: what it serves,
how its layers stand, and the command to run next. Use `--json` for structured output.
The [data guide](../../docs/content/software/data.md) explains the model.
The [data contract](../../specs/obc-data.md) defines bytes and admission checks.

## Setup

Prepare Rust, Python with `uv`, GEOS and Osmium before an update; `obc data` does not install them.
A changed producer does not rebuild its layer. Raise the `revision` of the layer in the change
that changes its output; a reader that needs new data raises its `format`.

`data/targets.toml` names the targets. `local` is a folder beside the store; `obc data update`
writes only a folder target. The settings of a target (region, optional layers, refresh
policies) are in the release that it serves, so they need no commit.

| Configuration or credential | Purpose |
| --- | --- |
| `data/targets.toml` | Targets and where they are |
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
| `obc data update local` | Bake what is stale, keep the rest, switch |
| `obc data update local --dry-run` | Review the update: what bakes and why, the cost, and its id |
| `obc data update local --expect ID` | Start the update only when its review still has this id |
| `obc data update local --add sun` | Switch an optional layer on; `--remove` switches it off |
| `obc data update local --refresh wikidata=30` | Set a refresh policy for the target |
| `obc data update local --only LAYER` | Bake a layer again after a code change with the same version |
| `obc data rollback local` | Serve the rollback release again; a second rollback undoes it |
| `obc data source refresh natural-earth` | Move a `digest` source to upstream's newest version |
| `obc data runs RUN --follow` | Observe progress |
| `obc data clean` | Review unused store bytes before `--apply` |

An update that stops keeps what it baked: run it again. The previous release stays as the
rollback of the target. Run one update or rollback of a target at a time: a collection can delete
a key that an update that runs at the same time counts on.
