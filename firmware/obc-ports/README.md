# obc-ports

Dependency-light `#![no_std]` semantic boundaries shared by the application, the platform
adapters and the hosts: fixes, GPS/calendar time, input events, recorded-track points and errors,
the capability polling traits, the `Sensors` bundle, and the `SettingsStore` contract. It owns no
drivers, buses, executor primitives, UI or render policy, and no allocation.

The manifest has no dependencies, and it must stay that way: `tools/dependency_rules.json` puts
`obc-ports` in the `foundation` allowlist, so a production edge to the app, the route algorithms,
a platform adapter, the board or the bootloader fails the dependency check. The checker also reads
the two standalone Cargo roots, so an edge from the board or the bootloader cannot evade it.

`DateTime` has no app year range. The 2020–2099 storage bounds live in
`obc_app::DateTimeEditorExt`.

From `firmware/`:

```sh
cargo test -p obc-ports --locked
python3 tools/check_dependencies.py
```
