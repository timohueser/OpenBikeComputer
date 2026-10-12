# Data operations

Run `obc data` from the repository root. Without a command it prints status.
Use `--json` for structured command output.
The [data guide](../../docs/content/software/data.md) explains the model.
The [data contract](../../specs/obc-data.md) defines bytes and admission checks.

## Setup

Prepare Rust, Python with `uv`, GEOS and Osmium before a build; `obc data` does not install them.
A changed producer does not rebuild its layer. Raise the `revision` of the layer in the change
that changes its output; a reader that needs new data raises its `format`. Follow the [Local app setup](../obc-data-steps/src/planner/README.md) for browser
assets, search dependencies and matching-host apps.

A started run detaches from the terminal. Publication runs on the machine that applies: it
needs rclone and the bucket credentials below. Live publishes from a pushed commit; Live
settings stay in the store and need no commit. One machine applies at a time. An apply builds services on the VPS,
stops them, installs the data and probes the new services. Allow a short outage. Before it replaces
a pointer, it keeps the old bytes at `runs/RUN/previous/KEY` in the store for a rollback.

| Configuration or credential | Purpose |
| --- | --- |
| `STORE/settings/pending.json` | Pending Live region definition, layers and refresh policy |
| `data/env/local.toml` | Local region and layers; ignored by Git |
| `STORE/regions/` | Saved Box or area selections; `data/regions/` supplies presets |
| `data/sources.toml` | Source declarations |
| `data/planner.toml` | Planner producer options |
| `data/planner-runtime.toml` | Runtime target and publication origins |
| `OBC_R2_BUCKET`, `OBC_R2_ACCESS_KEY_ID`, `OBC_R2_SECRET_ACCESS_KEY` | Production bucket and key |
| `OBC_R2_ACCOUNT_ID` or `OBC_R2_ENDPOINT` | Cloudflare account or explicit endpoint |
| `OBC_FIXTURE_R2_*` | Separate fixture bucket and credentials, with the same suffixes as production |
| `OBC_DATA_STORE` | Build store |
| `OBC_PLANNER_HOST` | VPS SSH address, `USER@HOST`; needs service and Caddy administration |
| `UV_PYTHON` | Selected prepared Python interpreter |
| `~/.cdsapirc` | CDS credential for a selected climate fetch |
| `~/.config/openbikecomputer/cdse-s3.env` | CDSE S3 credentials for a selected snow fetch |

Keep secrets in the private host environment or ignored `tools/obc.local`, never tracked config.
Plan names missing credentials before input preparation. Verified retained inputs remain usable.
Publication also requires its own bucket credentials.

## Daily commands

| Command | Action |
| --- | --- |
| `obc data` | Show what is live and what needs attention |
| `obc data plan live --json > PLAN` | Save the current review after required input preparation |
| `obc data prepare live --json` | Return a retained input-preparation handle |
| `obc data runs RUN --follow` | Observe progress |
| `obc data runs RUN --result` | Read the final output; preparation includes `.plan` |
| `obc data apply live --plan PLAN --yes` | Execute the exact owner-approved plan |
| `obc data runs RUN --stop` | Drain admitted work; an apply stops before its next phase |
| `obc data clean` | Review unused store bytes before `--apply` |
| `obc data dev` | Prepare Local from working-tree code and saved source versions |
| `obc data dev REGION --check --json` | Review pending Local work without source requests |
| `obc data dev REGION --prepare --json` | Return a retained Local preparation handle |
| `obc data dev --app map-builder --start` | Start the prepared Local Map builder |
| `obc data dev --app simulator --start` | Start the prepared Local Simulator |

Preparation acquires missing Wikimedia facts and checks stale identities by policy.
Pinned content compilation makes no Wikimedia requests. Fetch results report current HTTP
attempts and response body bytes; retained output bytes are separate. Historical transfers
are unknown. `dev --refresh-live` selects published Live versions; it does not check Wikimedia freshness.
Preparation leaves stopped apps stopped. Use the Local app commands to start one.
An incomplete plan needs preparation and a new review. Apply does not commit configuration.
A failed or stopped apply leaves live as it was until its pointer switch. Apply the plan again:
it uploads only what R2 lacks. `obc data status --check` compares R2 with live and lists the
keys of earlier releases that the next apply removes.

Live edits with `region live`, `layer live` and `policy` stay pending until apply records them
in each release. `obc data undo live` restores the applied region, layers and policies. Saved
region definitions stay.

## Agents

Use the plan and retained-operation commands above. Report the executing host,
selected environment, exact source moves, build work, `plan.remove` by prefix and publication
outcome. Keep original provenance when adopting portable data; do not create a foreign build
receipt. See [portable Local data](../../specs/obc-data.md#local-portable-data).
