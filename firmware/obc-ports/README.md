# obc-ports

Dependency-light `#![no_std]` semantic boundaries shared by the application,
platform adapters, and hosts. The crate owns fixes, GPS/calendar time, input
events, recorded-track points/errors, capability-specific polling traits, and
the `Sensors` bundle. It also owns the single `SettingsStore` contract, whose
associated `Value` keeps the foundation independent of the app's settings
model. It owns no drivers, buses, executor primitives, global mailboxes,
UI/render policy, or allocation.

`obc-app` and `obc-route` currently re-export compatibility names. FAR-04
([#797](https://github.com/timohueser/OpenBikeComputer/issues/797)) owns moving
board and host imports/direct dependencies to this crate; the compatibility
re-exports keep existing callers source-compatible in the meantime.

`DateTime` exposes semantic Gregorian arithmetic (`add_minutes`, signed UTC
offsets, Unix conversion) without an app year range. OpenBikeComputer's
2020–2099 storage/editor bounds, sanitising, and wrapping steppers live in
`obc_app::DateTimeEditorExt`; those UI-policy methods are intentionally not
inherent methods on the foundation value.

The manifest has no dependencies. The workspace classifies `obc-ports` in the
`foundation` allowlist in `tools/dependency_rules.json`, which rejects
production edges to core algorithms, the app, platform adapters, and hosts.
The checker also loads the excluded board and bootloader Cargo roots, so edges
to either standalone package cannot evade the allowlist.

From `firmware/`:

```sh
cargo test -p obc-ports --locked
python3 tools/check_dependencies.py
```
