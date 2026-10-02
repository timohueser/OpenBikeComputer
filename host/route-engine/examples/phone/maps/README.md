# Overlay rendering benchmark

Use the parent README for app installation and map files. Build the overlay
workload, then capture its exact API replies:

```sh
node host/route-engine/examples/phone/maps/build.mjs BW/maps CUTOUT/maps OUTPUT --overlays
python3 host/route-engine/examples/phone/maps/serve.py OUTPUT BW/maps CUTOUT/maps --capture http://127.0.0.1:8787
```

Open `http://127.0.0.1:8795/map-benchmark/index.html`. Completion writes
`OUTPUT/result.json`. Stop the server and rebuild to hash the captured replies.
Copy `OUTPUT` to `Documents/map-benchmark` and launch the app with `--maps`.
The app rejects unknown queries. No routing server runs on the phone during this
renderer workload. Compare SQLite query time separately. Run the server without
`--capture` to replay the same replies on the host.

For a RAM trace, set `audit` to `false` in `OUTPUT/config.json` after building.
This skips benchmark-only source cloning and hashing. Run the full audit first.
