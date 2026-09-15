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
measurement. Browser validation is recorded with its own result artifact.

Deliberately omitted: full workspace acceptance, physical hardware runs, board
resource builds, UI snapshot sweeps, wake-profile isolations, and country-scale
assembly. No public behavior or `docs/content/` documentation changed. The new
files document an opt-in development measurement.
