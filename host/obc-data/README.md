# Data operations

Run `obc data` from the repository root. It opens the keyboard interface in a terminal;
without a terminal it prints status. Use `--json` for structured command output.
The [data guide](../../docs/content/software/data.md) explains the model.
The [data contract](../../specs/obc-data.md) defines bytes and admission checks.

## Setup

Prepare the selected Rust toolchain, native compiler and producer tools before a build.
Discovery does not install them. Unsupported compiler settings or missing providers refuse
new work. Follow the [Local app setup](../obc-data-steps/src/planner/README.md) for browser
assets, search dependencies and matching-host apps.

Retained workers detach from the terminal. Publication runs on the machine that applies: it
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
| `obc data` | Open the interface |
| `obc data plan live --json > PLAN` | Save the current review after required input preparation |
| `obc data prepare live --json` | Return a retained input-preparation handle |
| `obc data runs RUN --follow` | Observe progress |
| `obc data runs RUN --result` | Read the final output; preparation includes `.plan` |
| `obc data apply live --plan PLAN --yes` | Execute the exact owner-approved plan |
| `obc data runs RUN --stop` | Drain admitted work; an apply stops before its next phase |
| `obc data clean` | Review unused store bytes before `--apply` |
| `obc data dev` | Prepare Local from working-tree code and saved source versions |

Preparation fetches missing Wikimedia captures and refreshes stale requests by policy.
Preparation leaves stopped apps stopped. Use the Local app commands to start one.
An incomplete plan needs preparation and a new review. Apply does not commit configuration.
A failed or stopped apply leaves live as it was until its pointer switch. Apply the plan again:
it uploads only what R2 lacks. `obc data status --check` compares R2 with live and lists the
keys of earlier releases that the next apply removes.

## Terminal controls

Checks and edits run in the background. An admitted check or edit finishes before `q` closes
the terminal. Run controls remain usable after checkout edits.

| View | Keys |
| --- | --- |
| Local | `2` opens it; `r` selects its region; Space changes optional layers |
| Local inputs | `R` checks saved data; `f` checks Live inputs over the network; `b` reviews work |
| Local apps | arrows select an app; `s` starts or stops; `o` opens a browser; `l` reads logs |
| Live region | `r` opens saved regions; `/` filters; Enter selects |
| Region editor | `n` creates an area selection; `b` creates a Box; `d` reviews deletion |
| Area selection | F5 loads the public area list; arrows and Space select; F3 shows selected areas |
| Region fields | Tab and Shift-Tab move; F2 saves the file; Esc keeps the draft |
| Sources | `f` changes scope; `/` filters; Enter shows details; `R` checks upstream |
| Source versions | `v` lists requests; Enter selects a source-wide move |
| Source policy | `e` opens presets; `c` enters 1..65535 whole days |
| Plan | `p` opens it; Space changes source moves; `d` shows steps; `f` prepares inputs; `b` builds |
| Apply | `a` reviews the machine, live changes and removals by prefix; `y` starts the exact reviewed plan |
| Run | Enter opens progress; `R` observes; `x` asks to stop admitted work; `y` stops |
| Prepared run | `p` reviews its returned plan before build or apply |
| Error | `!` opens the full message and fix; `x` dismisses it outside an input or Run |
| Current Rust code | F6 drains the check, restores the terminal and launches a fresh worker |


Live edits stay pending until apply records them in each release. `u` restores the applied
region, layers and policies. Saved region definitions stay. First entry into Regions or Local does not fetch or build. Required Plan
rows always apply; only source moves have checkboxes. Esc hides a Run and `q` quits its viewer;
neither stops the retained operation.

## Agents

Use the same plan and retained-operation APIs as the interface. Report the executing host,
selected environment, exact source moves, build work, `plan.remove` by prefix and publication
outcome. Keep original provenance when adopting portable data; do not create a foreign build
receipt. See [portable Local data](../../specs/obc-data.md#local-portable-data).
