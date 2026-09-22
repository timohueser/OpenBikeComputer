# Companion core

The companion app's Rust core: verified network and terrain cells assembled into a map in memory,
and routes planned over it by `obc-route` at the device's node limit. `src/ffi.rs` is the
hand-written C ABI, `include/obc_companion_core.h` declares it, and `include/module.modulemap`
lets the `OBCRouting` package (`companion-ios/Packages/OBCRouting`) do `import OBCCompanionCore`.

## Build

Needs macOS with Xcode and the iOS targets from `rust-toolchain.toml`. Run from any directory:

```sh
obc companion-core                        # all three slices into target/OBCCompanionCore.xcframework
obc companion-core aarch64-apple-darwin   # the Mac slice alone, for `swift test`
obc companion-core aarch64-apple-ios-sim  # the simulator slice alone
```

A named slice is rebuilt and the other slices on disk are kept.

**The `OBCRouting` package and the app need the XCFramework first.** Without it, the package
manifest stops and names `obc companion-core`. Nothing in Xcode rebuilds it after a Rust change:
run `obc companion-core` again. The suite commands and `obc ios-companion` pack it themselves.

## Test

```sh
obc test -p obc-companion-core
```

`tests/route.rs` routes over the web builder's cell fixture and pins the result in
`tests/route-vector.json`. The `OBCRouting` `CellRouterTests` suite checks the Swift side against
the same vector. After a deliberate router change, rewrite it:

```sh
OBC_REGENERATE=1 cargo test -p obc-companion-core
```

Linux CI type-checks the crate for `aarch64-apple-ios` through the `ci.ios-host-portability` suite.
