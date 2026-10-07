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

For retained Linux work, prepare systemd user services, linger and a private environment file
as in the [Linux bake setup](src/operation/README.md). macOS workers detach from the terminal.

For publication, prepare one Linux VPS owner with rclone, rsync and SSH. Install its matching
MIT `obc-data-plumbing` at `/opt/obc-data/bin/obc-data-plumbing`. Put its private environment
in `/etc/obc-data/owner.env`. Give it the same publication bucket as the initiating machine.
Only that owner account may change `/var/lib/obc-data/incoming` and `/var/lib/obc-data/store`,
and it must admit the fixed system service through systemd. Set `OBC_COMMIT_HOST=local` there.
On the laptop, select that owner with its configured SSH host.

Planner publication also needs the configured Linux CPython and Node versions, their host
libraries, systemd and Caddy. Add its API virtual host to `/etc/caddy/Caddyfile`. The owner
controls `/opt/obc-planner/services`, `/etc/systemd/system`, `/etc/caddy/planner/data` and
the downloads state directories. Set limits through the Linux setup before automatic work.

| Configuration or credential | Purpose |
| --- | --- |
| `data/env/live.toml` | Published region and optional layers |
| `data/env/fixtures.toml` | Exact fixture package selection and source versions |
| `data/env/local.toml` | Local region and layers; ignored by Git |
| `data/regions/` | Saved Box or area selections |
| `data/sources.toml` | Source declarations and refresh policy |
| `data/planner.toml` | Planner producer options |
| `data/planner-runtime.toml` | Runtime target and publication origins |
| `OBC_R2_BUCKET`, `OBC_R2_ACCESS_KEY_ID`, `OBC_R2_SECRET_ACCESS_KEY` | Production bucket and key |
| `OBC_R2_ACCOUNT_ID` or `OBC_R2_ENDPOINT` | Cloudflare account or explicit endpoint |
| `OBC_FIXTURE_R2_*` | Separate fixture bucket and credentials, with the same suffixes as production |
| `OBC_COMMIT_HOST` | Fixed publication owner: configured SSH host, or `local` on that owner |
| `OBC_DATA_STORE` | Initiating build cache; not the fixed owner state |
| `OBC_RUN_ENV_FILE` | Absolute private Linux worker environment file |
| `UV_PYTHON` | Selected prepared Python interpreter |
| `~/.cdsapirc` | CDS credential for a selected climate fetch |
| `~/.config/openbikecomputer/cdse-s3.env` | CDSE S3 credentials for a selected snow fetch |

Keep secrets in the private host environment or ignored `tools/obc.local`, never tracked config.
Missing source credentials block only a selected new fetch; verified retained inputs remain usable.
Publication also requires its own bucket credentials and owner admission.
Follow the [fixture setup](../../fixtures/README.md) for its isolated bucket and exact source packages.

## Daily commands

| Command | Action |
| --- | --- |
| `obc data` | Open the interface |
| `obc data plan live --json > PLAN` | Save the current review after required input preparation |
| `obc data prepare live --json` | Return a retained input-preparation handle |
| `obc data runs RUN --follow` | Observe progress |
| `obc data runs RUN --result` | Read the final output; preparation includes `.plan` |
| `obc data apply live --plan PLAN --yes` | Execute the exact owner-approved plan |
| `obc data runs RUN --stop` | Drain admitted local work before handoff |
| `obc data runs RUN --reconcile` | Record a verified final owner reply |
| `obc data clean` | Review unused store bytes before `--apply` |
| `obc data dev` | Prepare Local from working-tree code and saved source versions |

Preparation leaves stopped apps stopped. Use the Local app commands to start one.
An incomplete plan needs preparation and a new review. Apply does not commit configuration.
After a disconnect, observe the original Run. A missing owner record does not permit a retry
or timeout takeover. Inspect `/var/lib/obc-data/store/runs/RUN.jsonl` for acknowledged writes.
An unresolved intent blocks another commit; do not delete it.

## Read-only report

`obc data status --check --json` checks R2 and the installed VPS services. Layer states compare
current source/configuration at the recorded producer target. This does not check the bake
host's current compiler or build providers. Installed readiness checks actual runtime files,
process bindings and opened data. Missing helper or read access is unavailable, not healthy.

Set `OBC_STATUS_HOST` to a separate SSH alias. Its key has only a forced read command:

```text
restrict,command="/opt/obc-data/bin/obc-data-plumbing live-status" ssh-ed25519 PUBLIC_KEY
```

The account needs read access to service files, process metadata and `systemctl show`.
It does not stage a helper or change a service. Install the reviewed downloads runtime first.
Use a separate bucket token with object-read and listing permissions only.

The weekly `data-status.yml` workflow uses `OBC_STATUS_SSH_HOST` and `OBC_STATUS_SSH_USER`
repository variables, plus `OBC_STATUS_SSH_KEY` and `OBC_STATUS_KNOWN_HOSTS` secrets.
Its read-only bucket secrets are `OBC_STATUS_R2_ACCESS_KEY_ID` and
`OBC_STATUS_R2_SECRET_ACCESS_KEY`; the bucket/account variables match the table above.
It keeps one attention issue, changes its body only when evidence changes, and closes it only
when all comparisons are complete and clear. It never bakes, publishes or deletes data.

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
| Automation | `s` details; `e` edits Daily/Weekly/Monthly/Custom; arrows select; Tab field; Enter reviews; `y` confirms |
| Automation host | `d` disables future runs; `b` reviews limits; the Live row shows cadence, next run and last result |
| Plan | `p` opens it; Space changes source moves; `d` shows steps; `f` prepares inputs; `b` builds |
| Configuration | `C` reviews Git changes; `e` edits the message; Enter reviews the commit; `y` commits; `R` refreshes |
| Config CLI | `config review --json > REVIEW`; `config commit --review REVIEW -m MESSAGE --yes`; no push |
| Apply | `a` reviews the machine, live changes and removals; `y` starts the exact reviewed plan |
| Run | Enter opens progress; `R` observes; `x` stops admitted local work; `c` reconciles an owner result |
| Prepared run | `p` reviews its returned plan before build or apply |
| Error | `!` opens the full message and fix; `x` dismisses it outside an input or Run |
| Current Rust code | F6 drains the check, restores the terminal and launches a fresh worker |


Region and policy edits save files for review. `u` resets only Live settings, not regions or
source policies. First entry into Regions or Local does not fetch or build. Required Plan
rows always apply; only source moves have checkboxes. Esc hides a Run and `q` quits its viewer;
neither stops the retained operation. Stop refuses after publication handoff.
Manual laptop apply does not need the Linux schedule controller.

## Agents

Use the same plan and retained-operation APIs as the interface. Report the executing host,
selected environment, exact source moves, build work and publication outcome.
Inspect `plan.approval` even for a no-change apply; `applied.approval` is separate from publication.
Unsupported automatic approval does not disable manual publication.
Keep original provenance when adopting portable data; do not create a foreign build receipt.
See [portable Local data](../../specs/obc-data.md#local-portable-data) and
[automatic approval](../../specs/obc-data.md#manual-automatic-approval).
