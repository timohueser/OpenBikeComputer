# iOS host

The iPhone host's device core: the shared firmware application and renderer over one persistent
card, with the phone's GPS, compass, barometer and battery on the sensor ports. `src/ffi.rs` is the
hand-written C ABI the app links, `include/obc_ios_host.h` declares it, and
`include/module.modulemap` lets Swift do `import OBCHost`.

## Build

Needs macOS with Xcode and the two iOS targets from `rust-toolchain.toml`. Run from any directory:

```sh
obc ios-host             # both slices, release, into target/OBCHost.xcframework
obc ios-host --sim-only  # the simulator slice alone
```

## Test

The crate is target-independent and is tested natively on Linux and macOS:

```sh
cargo test -p obc-ios-host
```

Linux CI also type-checks it for `aarch64-apple-ios` through the `ci.ios-host-portability` suite.
