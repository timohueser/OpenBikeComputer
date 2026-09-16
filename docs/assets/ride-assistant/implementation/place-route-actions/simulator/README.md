# Real offline route-mode validation

Production source: `b9807e09c9d523cb6d3d66d9b410061d9d617dcc`. The checkout advanced to `d7dcaedf4b8befd35e86c5c568d76aff40b95b4a` during compilation; only the recorded resource baseline changed. See [build-identity.json](build-identity.json) for the pinned simulator hash. The later source `77fe2b79f` changes pending ownership and error handling. These captures remain evidence for the tested `b9807e09c` source.

The tests use the real Swiss map and a fixed GPS position at 46.723126, 8.194551. Five normal GPS ticks settle the active Meiringen route at progress 1664 m. The map is opened from an isolated flat card. `sandbox-exec` denies network access for every run. Each `*-command.json` contains the exact argument vector and each matching log records its result. No Assistant fixture controls, UI sweep, resource build, or board build were used for these checks.

## Results

- `resupply-detour-preview.png`: Migros detour. The complete stored route is 4964 m; the preview ends at the rejoin after 1859 m. The descriptor retains source progress 1664 m and rejoin 3422 m. The read-only inspector verifies the original prefix and tail.
- `resupply-routehere-preview.png`: Route here produces a 1045 m direct route with 28 vertices and no VisitDescriptor. The last point is exactly the Migros map coordinate, 8.185145, 46.728646. There is no retained original continuation.
- `resupply-back-to-detour.png`: a second Step restores the detour. Its OBCR payload is byte-identical to the initial detour.
- `resupply-routehere-accepted.png`: Press accepts the direct route. Its payload stays identical. The durable checkpoint uses object 9 and `original: None`, with upper progress 1045 m.
- `resupply-routehere-saved-routes.png`: the saved Routes page shows only the original Meiringen route. The accepted internal route remains on the card with the accepted flag for active navigation and recovery.
- `resupply-routehere-cancelled-settled.png`: Back leaves the original active route. The frame is byte-identical to the original active-map capture. Its hash and exact command are retained here; the duplicate PNG is omitted. After normal cleanup, only the original route remains and no Assistant checkpoint remains.

The original route payload has the same SHA-256 in every card. `assertions.json` records these comparisons. The small `*-geometry.log` files provide the retained-route evidence. [inspect_card.rs](inspect_card.rs) mounts cards read-only and checks their CRC and route geometry. The large cards, binaries and duplicate route exports remain local and are not included in this evidence bundle.

## Reproduce

Build from the recorded source with:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo build --release --locked -p obc-sim
```

Use the map with SHA-256 `ebe53f369a558e2c4e8da593b05433e55f730d83d07a928d46f7a236ec145bba`. Use the committed [Meiringen route](../../active-place-search/simulator/meiringen-loop-dem.obcr) and [stationary GPS track](../../active-place-search/simulator/stationary.gpx). [inputs.json](inputs.json) records all input hashes and sizes. Create an empty work directory, copy the route into its `routes/` directory, and seed a card:

```sh
sandbox-exec -p '(version 1)(allow default)(deny network*)' target/release/obc-sim "$MAP" --create-card "$WORK/base.obc" --routes-dir "$WORK/routes"
```

For each scenario, clone that fresh base card with `cp -c` on macOS (or copy it normally). Load the corresponding `*-command.json`, change the simulator, card, tracks, GPX and PNG paths to local paths, and run the argument vector. Keep the script and the expected screen unchanged. Scripts use only normal production inputs: `p` Press, `d` Step, `b` Back, `B` BackHold, `A` Up+Select hold, `T` GPS tick, `w` 800 ms of UI-clock advance, and `f` normal settle/render. The repeated `f` inputs allow route and storage work to finish; they do not change GPS or clock time.

The saved Routes check opens the main menu with `B` and selects its initial Routes row with `p`. Category selection uses absolute index 3 for Resupply. All scenarios start from the fresh base card, so one run cannot change another run's recovery or route state.

The original scenario seed command was:

```sh
/usr/bin/sandbox-exec -p '(version 1)(allow default)(deny network*)' /Users/timo/Documents/OSM-agents/ride-assistant-speed/.artifacts/place-preview-followup/obc-sim /Users/timo/.cache/openbikecomputer/fixtures/by-id/sim-assistant-meiringen/meiringen.obcm --create-card /Users/timo/Documents/OSM-agents/ride-assistant-speed/.artifacts/place-preview-followup/base.obc --routes-dir /Users/timo/Documents/OSM-agents/ride-assistant-speed/.artifacts/place-preview-followup/routes
```

The base card holds map object 1 and original route object 2, revision 1. It was seeded before the final simulator build, then cloned for each final run. All six flow commands use the final pinned simulator identified above.
