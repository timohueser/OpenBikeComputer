# Native planner providers

The Companion app compiles the Swift files in this directory. `RouteProvider`
calls the planner-service static library. `PMTilesArchive` reads the map and route
network tiles of an installed map. Follow
[the iOS on-ramp](../README.md) to build the app.

Run from the repository root to compare the PMTiles reader with the pinned
upstream writer:

```sh
bash companion-ios/PlannerNative/test-tiles.sh
```

## Companion download service

Use a canonical grid release from `obc planner grid`. Each planner release has
its own download service beside its routing and search services: `obc data serve`
starts them. The app calls `downloads` of the active release in the planner pointer.
A pointer without it names the legacy service at `/planner-offline/`: the unit
`obc-planner-downloads` on port 8790.

The VPS holds selection metadata; published payloads stream from R2. On the VPS,
`journalctl -u obc-data-serve` shows failures. The service evicts old
selection metadata within its cache budget. A phone can select an expired
area again. Installed maps remain usable.
