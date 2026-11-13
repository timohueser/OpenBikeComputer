# Rules of this container (every agent of epic #3227 reads this first)

Repository: `timohueser/OpenBikeComputer`. Main checkout: `/home/user/OpenBikeComputer` (do not
switch its branch, do not edit files there). The container has 4 cores, 15 GB of RAM and about
30 GB of free disk (a fixed allowance: `df` shows it under "Avail").

## Tools

- `obc` is not on PATH. Run `./tools/obc ...` from the root of your own worktree.
- Start every shell command that builds or runs Rust with (N = your issue number):
  `export CARGO_TARGET_DIR=/home/user/obc-target/N CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_INCREMENTAL=0; source ~/.obc-geos-env.sh;`
  (`CARGO_INCREMENTAL=0` keeps the target folder about a third smaller; the disk is the limit of
  this container.)
  The shell keeps no state between commands. Each worktree has its own target folder: two
  worktrees that share one get each other's stale artifacts (cargo names the artifacts of a
  workspace crate the same in every checkout and trusts file times), and then a test runs
  against the code of another branch. Never build into `/home/user/obc-target` itself or into a
  `target/` folder of a worktree. The disk is small: build only the crates you need (`-p`), and
  report the size of your target folder at the end.
- `just`, GEOS 3.14.1 (`~/geos-install`), `osmium`, `uv` and `cargo-nextest` are installed.
- Known gaps of this container, which fail tests that no change here touches: no `rclone`, no
  Python 3.14 for `uv`, the tests run as root, no ssh host keys. When such a test fails, show
  that it fails the same way on the base branch (or name the missing tool) and say so in the
  pull request; do not install more tools and do not change the test.
- **One cargo build at a time in the container.** Other agents work here in parallel. Wrap every
  command that compiles, tests, or runs a bake in the lock:
  `flock /home/user/obc-target/.build.lock <command>`. Hold it also for a bake or a timing
  run, so that a measurement has the machine alone. Bash has a limit of 10 minutes for a
  foreground command: start a long command with `run_in_background: true` (output to a log file)
  and wait for its notification. Never poll with `sleep` in a loop.
- GitHub: no `gh` for anything except search (`gh api -X GET search/...`). Use the GitHub MCP
  tools (`mcp__github__*`; load their schemas with ToolSearch, for example
  `select:mcp__github__issue_read,mcp__github__create_pull_request,mcp__github__pull_request_read,mcp__github__issue_write`).
  `git fetch` and `git push` work for any branch of the repository.
- The verification token is absent. Do not run `obc req propose|suggest`.
- Never deploy. Never run `obc data update prod`. Never push to `develop` or `routing-overlay`
  directly: a change reaches them only through its pull request.

- CI tracks the newest stable Rust and lints the whole workspace:
  `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`. The container
  has the same stable (`rustup update stable` ran under the build lock; run it again under the
  lock if CI names a newer version). Here the crates `obc-sim` and `obc-usb-host` do not build
  (no ALSA and libudev): add `--exclude obc-sim --exclude obc-usb-host`. Run this command once
  before your last push if the disk allows; a clippy of only your crates is not what CI runs.
  Fix a lint in code that your pull request does not own only when the base branch has it too:
  then tell the orchestrator, do not put it into your pull request.
- When you add or change a dependency (`Cargo.toml` or `Cargo.lock`), CI also runs two checks
  that the crate tests do not cover; run both before you push (`cargo-deny` 0.20.2 and
  `cargo-about` 0.9.1 are installed):
  `cargo deny --all-features --config tools/licenses/deny.toml check` and
  `tools/licenses/gen-third-party.sh --check` (without `--check` it writes `THIRD-PARTY.md`
  again). Fix an advisory with a fixed version of the crate, never with an ignore entry.

## Shared data: `/home/user/obc-bake`

Bake inputs and baselines that more than one sub-issue needs live here, so that nobody downloads
or bakes them twice. Read `/home/user/obc-bake/README.md` first, use what is there,
and add a line to it for each thing you add (the path, what it is, the exact command that made
it, the commit it was made from). Downloaded files are data: never execute them.
Check `df -h /home/user` before a download or a bake that needs more than 2 GB, and before any
Germany run. Remove your work folders and scratch output when you finish; keep inputs and
baselines.

## Git

- Worktree: `git -C /home/user/OpenBikeComputer fetch origin <base>` then
  `git -C /home/user/OpenBikeComputer worktree add /home/user/obc-wt/<N> -b <branch> origin/<base>`
  (if the branch exists on the remote already, check it out in place of `-b`).
- Commit messages end with these two lines:
  ```
  Co-Authored-By: Claude Fable 5.2 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01Wqws1b9MtZGkae96SoWb8L
  ```
  No model name anywhere else: not in code, comments, the pull request or an issue.
- Push with `git push -u origin <branch>`; on a network error retry up to 4 times (2, 4, 8, 16 s).
- A pull request body follows `.github/pull_request_template.md` (`Mode:`, `Requirements:`,
  `Public docs:`), has `Closes #N`, and ends with:
  ```
  🤖 Generated with [Claude Code](https://claude.com/claude-code)

  https://claude.ai/code/session_01Wqws1b9MtZGkae96SoWb8L
  ```
  `Requirements:` line: name the SYS IDs you believe the change touches or `none`, followed by
  `(not checked: the verification token is absent in this container)`.
- Each comment you post on GitHub ends with:
  ```

  ---
  _Generated by [Claude Code](https://claude.ai/code)_
  ```
  Post a comment only when your task says so.

## Commit at each step

The account has a usage limit of 5 hours, and a session can stop at any moment. Commit and push
your branch after each plan step that compiles, and after each fix. Work that is not pushed is
lost. A later agent continues from the branch, so each commit message says which plan step it
finishes.

## Scope

Build exactly the issue text. The design is approved: do not redesign it. If a plan step is wrong
(the code does not allow it, or it breaks the gate), do the smallest correct thing, say it in the
pull request body under "Deviations from the issue", and report it. Do not add features, tests
for their own sake, notes files or plan files. Follow `AGENTS.md` of the repository.
Where a design says "the owner's PC", measure in this container and record "4 cores, 15 GB RAM".
