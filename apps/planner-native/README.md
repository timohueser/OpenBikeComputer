# Native planner providers

The Companion app compiles the Swift files in this directory. `RouteProvider`
calls the route-server static library. `OverlayProvider` reads overlay cells.
`PMTilesArchive` reads map tiles from an installed map. Follow
[the iOS on-ramp](../../companion-ios/CLAUDE.md) to build the app.

Run from the repository root to compare the PMTiles reader with the pinned
upstream writer:

```sh
bash apps/planner-native/test-tiles.sh
```

## Companion download service

Use a canonical grid release from `obc planner grid`. The standard
`obc planner deploy` command installs the download service with the online
services. To replace only its metadata, run from the repository root:

```sh
python3 -m tools.planner_downloads_deploy --host USER@VPS --source RELEASE --max-cache-bytes 268435456
# Repeat with --apply to install and start the service.
```

The service uses port 8790 on loopback and the existing Caddy planner import.
The VPS holds selection metadata; published payloads stream from R2. Inspect
`journalctl -u obc-planner-downloads` for failures. The service evicts old
selection metadata within its cache budget. A phone can select an expired
area again. Installed maps remain usable.
