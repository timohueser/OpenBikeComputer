# Builder core

Build from `builder/web`. Install `wasm-pack` first.

```sh
npm ci
npm run build:wasm
npm run check
npm test
```

| Output | Use |
| --- | --- |
| `../web/src/lib/core/pkg` | Production conversion, assembly, preview, grid, GPX, SHA-256, and store client APIs |
| `../web/test-support/flat-device/pkg` | The same crate with `test-device`, for browser tests and the dev harness |

Both outputs are generated. The test-device package must stay outside production imports.
The app loads the production core before it imports synchronous data APIs. Workers load their own instance.

Run native checks from the repository root:

```sh
obc test -p obc-builder-bridge
cargo clippy -p obc-builder-bridge --all-targets --all-features -- -D warnings
python3 builder/wasm/wasm_size_guard.py
```

The fixture cutter needs GEOS. It is a dev dependency and must stay outside the production graph.
The native flat-device library stays in `sim/flat-device`.
