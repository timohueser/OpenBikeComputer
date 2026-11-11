---
system: data-registry
kind: system
area: Data and services
purpose: "Declares the external sources, regions, targets and options that `obc data` reads."
owns:
  - data/**
depends_on: []
test_depends_on: []
status: approved
---

# Data registry

## What it does

The data registry is a set of TOML and JSON files in `data/`, with no code. `obc data` and the data steps read them.

- [`sources.toml`](../../data/sources.toml) declares each external dataset, asset and tool: its licence and credit, where to fetch it, and when to fetch a newer version.
- `regions/` holds the region presets: a box or an area, with countries and a time zone.
- [`targets.toml`](../../data/targets.toml) names the places that releases go to: a local folder or the public bucket.
- `archive/` describes the datasets of the archive. `planner.toml` and `planner-runtime.toml` hold the options of the planner. `machines.toml` names the build machines for the operator page.
- [`trailview/`](../../data/trailview/README.md) holds the rules of the TrailView ground and features.

The data engine reads the registry with [`sources.rs`](../../host/obc-data/src/sources.rs). Python steps read it with [`data_registry.py`](../../tools/data_registry.py). The [data contract](../../specs/obc-data.md) defines the format.

## Rules that must hold

| Rule | Check |
| --- | --- |
| A data source with no licence is blocked. | `a_missing_licence_blocks_but_credentials_only_gate_selected_fetches` in [`sources.rs`](../../host/obc-data/src/sources.rs) |
| A licence is an SPDX expression. | `a_licence_is_an_spdx_expression` in [`sources.rs`](../../host/obc-data/src/sources.rs) |
| The registry in the repository loads. | `the_checked_in_registry_loads` in [`sources.rs`](../../host/obc-data/src/sources.rs) |
| Only the OSM sources and the prepared sources refresh by age. | `only_osm_and_prepared_sources_refresh_by_age` in [`sources.rs`](../../host/obc-data/src/sources.rs) |

## Decisions

- **Data changes only on purpose.** The OSM sources refresh by age, so that the roads stay current. A prepared source changes only when a person prepares a newer version.
