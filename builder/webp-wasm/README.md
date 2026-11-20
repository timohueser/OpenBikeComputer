# WebP bridge

`obc-webp-bridge`: the exact WebP decode of TrailView's workers behind `wasm-bindgen`. The ground
worker loads it where the browser's decode fails its probe; the terrain decode loads it from the
first terrain tile whose browser decode has an alpha other than 0 or 255.

## Build and test

```sh
obc exec bash builder/build-wasm-bridges.sh       # all bridges; this one goes to builder/web/src/lib/trailview/stream/webp
python3 builder/wasm/wasm_size_guard.py --module webp
obc test -p obc-webp-bridge
obc exec cargo clippy -p obc-webp-bridge --all-targets -- -D warnings
obc exec cargo clippy -p obc-webp-bridge --target wasm32-unknown-unknown -- -D warnings
```
