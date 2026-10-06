# Shared map types and bytes

Run these commands from the repository root:

```sh
./tools/obc test -p obc-map-core
cargo clippy -p obc-map-core --all-targets -- -D warnings
```

Generate the checked config schema with:

```sh
cargo run -p obc-pack --bin obc-pack -- schema --config > host/obc-map-core/schema/config.schema.json
```
