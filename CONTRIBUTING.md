# Contributing

The project is an active prototype. Get in contact before a large change; small fixes can go
straight to a pull request against `develop`.

- [CLAUDE.md](CLAUDE.md) is the working agreement for humans and agents: build and test
  commands, what gets recorded where, and how reviews work.
- [docs/testing.md](docs/testing.md) explains the test plan and the CI artifacts.
- The nearest README has each surface's setup. `./tools/obc` lists the development tasks;
  `obc help TASK` describes one.

Before a push, `obc ready --base origin/develop` runs the gates your change selects and prints a
pull-request skeleton. Every pull request states the checks it ran and ends with a
`Requirements:` line.
