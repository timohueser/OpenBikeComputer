# Contributing

The project is an active prototype. Contact the owner before a large change. Send small fixes
as a pull request against `develop`.

## Setup

Start with [simulator setup](README.md#try-the-simulator) to clone the repository, install the
shared tools, and run the application without hardware. Run the commands below from the checkout
root. `./tools/obc` works without an installed alias; `./tools/obc help TASK` describes a task.

Use each surface's README for its extra setup and checks: [simulator](apps/obc-sim/README.md),
[board firmware](firmware/obc-fw-nrf54l/README.md), or [web demo](apps/obc-web-demo/README.md).

## Check your change

For a workspace Rust crate:

```sh
./tools/obc test -p obc-app
cargo clippy -p obc-app --all-targets -- -D warnings
cargo fmt --all
```

Replace `obc-app` with the crate you change. For a standalone Cargo root or a non-Rust surface,
use its README's checks. [The test guide](docs/testing.md) explains suites and fixtures.

Before a push:

```sh
./tools/obc ready --base origin/develop
```

This runs the selected gates and prints a pull-request skeleton.

## Open a pull request

Describe the problem and the resulting behavior. List the checks you run and the checks you leave
out. Say whether public docs change; put those changes in their own `docs:` commit.

List affected `SYS-*` IDs on the `Requirements:` line, or write `Requirements: none`.
Find the requirements in the [verification console](https://releases.openbikecomputer.com).
