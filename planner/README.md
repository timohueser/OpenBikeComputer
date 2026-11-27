# Planner

The services behind the route planner: the route engine, the builder of its graph, the HTTP and
phone wrapper around the engine, place search with the sentence query model, and the tile Worker.

Not here: the planner UI (`builder/web/src/components/planner/`,
[setup and deployment](../builder/web/src/components/planner/README.md)), the iOS providers
([`companion-ios/PlannerNative`](../companion-ios/PlannerNative/README.md), `PlannerSearch`),
and the data pipeline that calls these crates (`obc data`, in `host/obc-data` and
`host/obc-data-steps`, with `tools/planner_*.py`). Data reaches the services from the store or
the bucket, never through this folder.

## Where to start

| Package | Runs | Open first |
| --- | --- | --- |
| [`router/`](router/README.md) (`planner-router`) | A library inside `service` and `router-build`. | `src/lib.rs`; the contract is `specs/route-api.md`. |
| [`router-build/`](router-build/README.md) | On the operator machine, as the `obc data` routing steps. | `src/step.rs`; `specs/route-package.md`, `specs/route-catalog.md`. |
| [`service/`](service/README.md) (`planner-service`) | On the VPS, one process per planner release, behind Caddy at `/planner-api/`. The same crate without the `http` feature is a static library in the iOS app. | `src/lib.rs`: `respond` is the one entry for HTTP and for the phone. |
| [`search/`](search/README.md) | On the VPS (Node server and a Python environment, one process per region). iOS bundles the same `runtime.mjs`. Its own npm root. | `server.mjs`, `runtime.mjs`; [`query/`](search/query/README.md) trains the model. |
| [`tiles/`](tiles/README.md) | A Cloudflare Worker on `tiles.openbikecomputer.com` that reads the R2 bucket. `src/local.mjs` serves the same tiles on loopback for `obc data serve`. Its own npm root. | `src/worker.mjs`; `specs/planner-release.md`. |

The three crates are members of the root Cargo workspace. The web planner calls the service over
HTTP; no browser code links the router today, though it compiles for `wasm32`.

## Run and test

```sh
obc test -p planner-router          # also planner-router-build, planner-service
obc data update local --only planner/routing   # rebuild the routing layer
obc data serve                      # the services and tiles of a local release
npm test --prefix planner/search    # also: npm run test:query
npm ci --prefix planner/tiles && npm test --prefix planner/tiles
```

Search needs Node 24, Python 3.14 and uv; `python3 planner/search/setup.py` restores the pinned
model. `npm run test:data` and `npm run test:model` need local data and are manual. CI runs the
search and tile suites in the job `planner-search`.

## Deployment

```sh
obc data deploy services --dry-run   # then: --expect COMMIT
obc data deploy tiles
```

`deploy services` builds a commit on the VPS in its own directory, restarts the follower
(`obc data serve prod`) and keeps one previous build for a rollback.
[`tools/vps/README.md`](../tools/vps/README.md) describes the machine. `deploy tiles` runs
`wrangler deploy` to production. Both act on production: do not run one without the owner's
request, and never run two publications or deployments at the same time.

## Gotchas

- Deploy the services before you publish a release in a new format. The follower does not start
  a release whose format the services cannot read, and `obc data update prod` waits for it, up
  to 24 hours, then leaves prod as it was.
- The format numbers must agree between code and data: the routing package (`FORMAT` in
  `router/src/cells.rs`), the search schema and grid (`search/federation.mjs`,
  `search/installation.mjs`) and the tile grids (`tiles/src/grid.mjs`). `--formats` on a server
  prints them and opens no data.
- Wrangler reads the checkout, not a commit, so `deploy tiles` refuses uncommitted changes in
  `planner/tiles`. `deploy services` refuses them in `tools/vps`, `data/planner-runtime.toml`
  and `.python-version`.
- Only `obc data deploy tiles` applies `wrangler.toml`. A Worker pasted in the dashboard has no
  rate-limit binding and applies no limit, and a `workers.dev` route bypasses the firewall
  rules of the zone.
- `planner-service` and `obc-data` build in two separate cargo calls on the VPS. One call would
  unify their features.
- The work folder of `router-build` must be on a disk, not tmpfs. A killed bake leaves
  `obc-router-build-<pid>-<n>` behind. The OSM input must be sorted by ID.
- The search server exits when inference hangs; it needs a supervisor.
- `search/query/contract.json` and `marks.json` are generated (`python3 schema.py`,
  `python3 lexicon.py marks`). A stale file fails a test and triggers web and iOS suites.
- After you copy a model by hand into `search/data/model/`, do not run `setup.py`: it restores
  the pinned model.
- The modules `directory` and `open` of the router do not exist on `wasm32`; a wasm host must
  supply a `package::Source`.

## Decisions

- The engine knows no transport, file system, rendering or itinerary, so one core serves HTTP
  and the phone, and can serve a browser.
- The service has one `respond`, so HTTP and the phone answer with the same code. The phone
  library has no `http` feature, so no HTTP stack ships in the app.
- Search returns edits to a plan and never calls routing. The client applies the edits and routes.
- `router-build` has three work layers (source, roads, costs), so a stopped bake or a new
  profile list repeats only the later steps.
- The Worker counts only a cache miss against its rate limit, because only a miss reads the
  bucket. It refuses a zoom above 16 before any bucket read, because no archive is deeper. It
  caches a missing object for five minutes only, because a release gets its objects before it
  goes live.
- The local tile server gives the same bytes as the Worker; `tiles/test/worker.test.mjs` holds it.
