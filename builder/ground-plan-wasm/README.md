# Ground plan bridge

`obc-ground-plan-bridge`: the cell build of [`obc-ground-plan`](../../host/obc-ground-plan/README.md)
behind `wasm-bindgen`, for the TrailView plan worker and the timing page. The viewer loads it only
with `ground=plan` in the address.

## Build and test

```sh
obc exec bash builder/build-wasm-bridges.sh       # all bridges; this one goes to builder/web/src/lib/trailview/plan/pkg
python3 builder/wasm/wasm_size_guard.py --module ground-plan
obc test -p obc-ground-plan-bridge
obc exec cargo clippy -p obc-ground-plan-bridge --all-targets -- -D warnings
obc exec cargo clippy -p obc-ground-plan-bridge --target wasm32-unknown-unknown -- -D warnings
```

## Check 7: native and WASM bytes

```sh
obc exec bash builder/ground-plan-wasm/check-bytes.sh
```

It writes the requests of every fixture cell into a temporary folder, builds each in Node with
the package, and fails on a plan digest or a far block that differs from the native one. The
native `requests` fails where a cell's far-only build gives another far block than its full build.

| File | What it holds |
| --- | --- |
| `check.mjs DIR` | Builds each request of `DIR/index.json` in WASM; prints `cell digest native-ms wasm-ms` |
| `check-bytes.sh` | `requests --all --runs 1`, then `check.mjs` |

## Timing page

```sh
obc exec cargo run --release -p obc-ground-plan --example debug -- \
  requests --places v06,v12,v17,i07-hairpin,b45-bridge-junction --out .artifacts/ground-plan/requests
cd builder/web && obc exec npm run dev -- --host
```

The `requests` command prints the address. Open
`http://<host>:5173/plan-timing.html?requests=/@fs/<repository>/.artifacts/ground-plan/requests/`.

| Parameter | Value |
| --- | --- |
| `requests` | The folder that holds `index.json` (required) |
| `runs` | Runs per cell after the cold run, 1 to 50 (default 5) |
| `cells` | `x-y` names, separated by commas (default: all) |

On an iPhone: join the home Wi-Fi of the PC, open the address with the PC's LAN address as
`<host>` in Safari, and wait for the summary line. The text area then holds the result as JSON;
select it to copy. The copy button works only in a secure context, such as `localhost`.

| Column | What it holds |
| --- | --- |
| `first` | The first run of the cell; only the first row runs on a cold instance |
| `mean` | All later runs over their count; use it where the clock is coarse (Safari: 1 ms) |
| `prepare` | Gunzip and WebP decode of the cell's fixture files in the worker, with its tile cache |
| `<stage> median`, `p95` | Per stage of the bridge, over the later runs |

## Pitfalls

- A `CellRequest` and a `CellPlan` hold WASM memory until `free()`. `build` and `far` take their
  request: a `free()` after them throws.
- `far(request)` gives a cell's far block alone, without faces, records or rim: the
  `CellPlan.far()` of a `build` of the same request. `CellPlan.far()` throws where the crate
  refused the far block; the records and the rim of that plan are good.
- An instance keeps a few feature tiles parsed. A later request names such a tile (`name_tile`).
  `missing()` lists the named tiles that the instance let go: give their bytes, or the build fails.
- `CellPlan.digest()` costs about as much as the build of the graph: the viewer asks for it only
  with the debug draw. It fails on a NaN, whose text hides its bits.
- WASM memory never shrinks. The viewer starts a new worker when the heap passes its limit.
