# Shared PBF source primitives

This crate owns ordered blob scans, integer crop bounds, source object sets and
relation polygon assembly. Callers own feature selection and graph construction.
It needs libGEOS 3.14 or later. Data workers require one shared GEOS C/C++ pair
on macOS or Linux. Restart the worker after replacing either loaded library.
An unsupported installation blocks GEOS producers; read-only commands stay available.

Run these commands from the repository root:

```sh
./tools/obc test -p obc-pbf -p obc-bake
cargo clippy -p obc-pbf --all-targets -- -D warnings
```
