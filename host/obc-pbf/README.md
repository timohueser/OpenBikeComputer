# Shared PBF source primitives

This crate owns ordered blob scans, integer crop bounds, source object sets and
relation polygon assembly. Callers own feature selection and graph construction.
It needs libGEOS 3.14 or later.

Run these commands from the repository root:

```sh
./tools/obc test -p obc-pbf -p obc-bake
cargo clippy -p obc-pbf --all-targets -- -D warnings
```
