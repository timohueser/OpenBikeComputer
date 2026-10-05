# Native planner providers

The Companion app compiles the Swift files in this directory. `RouteProvider`
calls the route-server static library. `PMTilesArchive` reads the map and route
network tiles of an installed map. Follow
[the iOS on-ramp](../../companion-ios/CLAUDE.md) to build the app.

Run from the repository root to compare the PMTiles reader with the pinned
upstream writer:

```sh
bash apps/planner-native/test-tiles.sh
```

## Companion download service

Use a canonical grid release from `obc planner grid`. `obc planner deploy`
installs the download service with the online services and switches it to each
new release before the catalogue activates that release.

The service uses port 8790 on loopback and the existing Caddy planner import.
The VPS holds selection metadata; published payloads stream from R2. Inspect
`journalctl -u obc-planner-downloads` for failures. The service evicts old
selection metadata within its cache budget. A phone can select an expired
area again. Installed maps remain usable.
