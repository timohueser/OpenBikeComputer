---
system: obc-data
kind: shell
purpose: "Runs the `obc data` command and its operator page."
owns:
  - host/obc-data-steps/**
  - builder/web/src/data.ts
  - builder/web/src/lib/data/**
  - builder/web/src/components/data/**
depends_on:
  - data-engine
  - device-map-baking
  - device-map-builder
  - foundations
  - map-assembly
  - route-planning
  - routing
  - search
  - source-preparation
  - trailview
test_depends_on:
  - map-reading
  - repository-tooling
status: approved
---

# obc data

## What it does

An operator runs `obc data` to build and publish the project's data: the map cells of the device, the releases of the route planner, and the prepared sources (processed copies of large external datasets). The binary `obc-data` ([`main.rs`](../../host/obc-data-steps/src/main.rs)) gives the data engine the list of products and their steps ([`lib.rs`](../../host/obc-data-steps/src/lib.rs)). The products are the device maps and the planner, with its routing, search and TrailView layers. The list also names the producers of prepared sources.

`obc data ui` serves an operator page on this machine, at port 8700 ([`ui.rs`](../../host/obc-data-steps/src/ui.rs)). The page is a Svelte app ([`DataApp.svelte`](../../builder/web/src/components/data/DataApp.svelte)) that the map builder's build configuration builds. For each action, the page sends one `obc data` command to the server, and the server runs it.

The data engine owns the commands, the store, the versions and the publication.

## Rules that must hold

| Rule | Check |
| --- | --- |
| The page answers only a browser on this machine or at the end of an SSH tunnel, only requests from the page itself, and only JSON posts. | `the_page_answers_only_this_machine_and_json_posts` in [`ui.rs`](../../host/obc-data-steps/src/ui.rs) |
| The page runs only the commands that its screens use. | `the_page_runs_an_allowed_command_and_answers_its_json_and_exit_status` in [`ui.rs`](../../host/obc-data-steps/src/ui.rs) |
| A credential goes to the standard input of the command, never to its arguments. | `a_credential_goes_to_standard_input_and_never_to_the_arguments` in [`ui.rs`](../../host/obc-data-steps/src/ui.rs) |

## Decisions

- **The page runs the command line.** Each action of the page is one `obc data --json` command. So the page can do only what the command line can do, and a credential never goes to the arguments or to a log.
