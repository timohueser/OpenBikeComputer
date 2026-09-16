# Baseline validation

The measurement implementation was checked on Linux. Commands ran from the
repository root, with the local GEOS environment loaded for direct Cargo checks.

Passed:

```sh
obc test -p obc-reader -p obcm-assemble -p obc-bench
cargo test -p obc-reader --features nav-metrics
cargo clippy -p obc-reader -p obcm-assemble -p obc-bench --all-targets --features obc-reader/nav-metrics,obcm-assemble/mem-profile,obc-bench/nav-metrics -- -D warnings
obc suites check
cargo fmt --all
cargo fmt --manifest-path firmware/obc-fw-nrf54l/Cargo.toml
cargo fmt --manifest-path firmware/obc-boot/Cargo.toml
cargo fmt --manifest-path apps/obc-desktop/Cargo.toml
python3 -m py_compile host/obc-bench/dev/navigation/run.py host/obcm-assemble/dev/navigation/native.py
```

The first direct Clippy invocation could not find GEOS. Loading the existing
`$HOME/.obc-geos-env.sh` fixed the build environment; the command then passed.

The route runner completed all six cases in three cold-plan samples each and
checked output repeatability. Native assembly completed three times with full
verification and independent SHA-256 readback. The retained JSON contains every
measurement. The persistent-context Chromium worker completed one matched assembly
with full verification and an independent output digest equal to native. The
browser runner passed `node --check`. Its executable build and run commands are
in the assembly README; `browser-baseline.json` retains its result and provenance.

Deliberately omitted: full workspace acceptance, physical hardware runs, board
resource builds, UI snapshot sweeps, wake-profile isolations, and country-scale
assembly. No public behavior or `docs/content/` documentation changed. The new
files document an opt-in development measurement.

The native runner's local-input preflight also passed against the 41 pinned
objects (248,096,256 bytes):

```sh
python3 host/obcm-assemble/dev/navigation/native.py target/release/obcm-assemble /tmp/obc-ng-inputs/data /tmp/obc-ng-preflight --check-inputs
```

This check performed no network request or assembly. No measurement was repeated.
Python syntax and `obc suites check` passed after the preflight change.
