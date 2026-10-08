# OSM source preparation

Run these commands from the repository root:

```sh
./tools/obc test -p obc-osm
cargo clippy -p obc-osm --all-targets -- -D warnings
```

Prepare Osmium on PATH, or set `OBC_OSMIUM` to its executable. The native fixture cases run
when that tool is present. They use the checked-in tiny PBF and need no source download.
The data step checks the executable checksum and version before and after extraction.
