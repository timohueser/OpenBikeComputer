# WebP bridge

`obc-webp-bridge`: the WebP decode of the TrailView ground worker behind `wasm-bindgen`. The worker
loads it only where the browser's decode fails the worker's probe.

## Build and test

```sh
obc exec bash builder/build-wasm-bridges.sh       # all bridges; this one goes to builder/web/src/lib/trailview/stream/webp
python3 builder/wasm/wasm_size_guard.py --module webp
obc test -p obc-webp-bridge
obc exec cargo clippy -p obc-webp-bridge --all-targets -- -D warnings
obc exec cargo clippy -p obc-webp-bridge --target wasm32-unknown-unknown -- -D warnings
```
