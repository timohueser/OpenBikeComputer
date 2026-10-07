# Local Web planner

Prepare Rust, the native C compiler, GEOS, Node 24+, Python 3.12+ and `uv`.
Follow the [builder core setup](../../../../builder/wasm/README.md) for the production
bridge and Wasm. Prepare dependencies from the repository root:

```sh
npm ci --prefix builder/app
npm ci --prefix apps/planner-search
npm ci --prefix planner/tiles
uv sync --locked --group search-runtime
```

The commands below do not install dependencies. Start from a current checkout.
On Linux, prepare the systemd user manager, linger and private `OBC_RUN_ENV_FILE`
as in the [data setup](../../../obc-data/README.md#operation).

```sh
obc data dev
obc data dev REGION
obc data dev --refresh-live
obc data dev --start
```

`data/env/local.toml` holds the region and optional layers. The first preparation
copies the Live settings. An explicit region updates Local. Preparation keeps the
saved Local versions. `--refresh-live` takes the current published versions.
The working tree supplies current producer code, region geometry and options.
Compatible portable layers keep their original provenance. Only changed layers build.
New builds need the tools and source credentials of their selected producers.
The route executable always builds for the current host.
Preparation leaves stopped apps stopped. It updates an already running owner.

| Command | Action |
| --- | --- |
| `dev --prepare` | Return a retained preparation run handle |
| `dev --start` | Start the last verified view; open the browser unless `--json` |
| `dev --stop` | Ask the owning supervisor to drain its children |
| `dev --open` | Open the ready planner at `http://127.0.0.1:5173/planner.html` |
| `dev --logs` | Read recent supervisor stderr |
| `dev --status` | Read service state without starting work |

Inspect or stop preparation with `obc data runs RUN`. Serving uses a separate
store lock. Preparation can run while the prior view serves. Startup replaces
only affected running children. Stop also removes obsolete known views after
drain. It retains the current prepared view and saved data collection root. Opening a terminal view starts no download or build.

Ports `5173`, `8780`, `8788` and `8789` must be free before the first start.
Readiness checks the opened routing package, search grid and query model.
Prepared frontend assets and HTTP responses do not prove a browser workflow.
See the [Local service contract](../../../../specs/obc-data.md#local-app-services).
