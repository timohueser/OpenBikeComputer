# Ride Assistant implementation handoff

**Status: Historical planning handoff.** Production implementation is present. Use the
[current simulator guide](../../../../apps/obc-sim/README.md#ride-assistant) and
[current architecture](../../../content/software/architecture.md) for current behavior.
The [epic](https://github.com/timohueser/OpenBikeComputer/issues/1734) remains open for acceptance;
check its outstanding items before claiming completion. The branch, machine paths, and commands
below describe the original planning session. They are not current setup instructions.

## Original handoff

The [epic](epic.md) is the current plan for [GitHub #1734](https://github.com/timohueser/OpenBikeComputer/issues/1734).
It replaces the earlier broad question shortlist. The accepted four screens are Find a place,
What's next, Landmarks and Easier route. Production implementation has not started in this
planning change. The current simulator still uses the reviewed mocks.

Read the epic dependency table, each assigned child, and the [review ledger](review.md) before
implementation. The [issue map](issues.json) maps each local specification to its GitHub issue. The GitHub issues
are the work tracker; this directory records the reviewed plan and handoff.

## Start a fresh implementation session

Use `codex/ride-assistant-landmarks` as the prototype source. The final reviewed layout commit is
`d963d667`. Inspect the current remote develop and prototype heads before choosing the integration
base. Do not trust the branch or commit in the earlier wireframe handoff. Preserve unrelated work.
Use the root AGENTS.md, CONTRIBUTING.md, docs/testing.md and the nearest package README.
Check that `obc` targets the selected checkout: the installed wrapper on this machine currently
uses `/Users/timo/Documents/OSM`. Use the selected checkout's repository tools when needed; a
check in another worktree is not evidence for the implementation head.

Implement small working slices in dependency order. RA01 supplies real source packages first.
RA02 place queries, RA03 route facts and RA08 landmark preparation can then proceed independently.
RA04 and RA05 own shared navigation acceptance and visits. Coordinate format changes through their
listed owners and consumer matrices before parallel edits. Build the board adapters as those
contracts land; do not defer their feasibility to the final simulator issue.

Keep each child open until its actual acceptance criteria pass. Record the commit and exact focused
checks in the child. A screenshot made from fixed costs or mock routes is not completion evidence.
Do not close the epic while physical acceptance is pending. Use named frames during development;
coordinate one final snapshot sweep and one final resource build under the repository budget.

The target is a simulator the owner can explore with real offline data, plus prepared device-test
artifacts. RA13 specifies commands, source hashes, scenarios, measured constraints and the physical
checklist. Do not claim an overnight run or scheduled device test exists merely from this plan.

## Decisions that bound this release

- Keep existing query, Navigator, planner, storage and rendering owners. No Assistant service stack.
- Known-closed places are hidden; unknown hours remain visible. Current local time uses the device's
  configured offset. No arrival-time prediction or new timezone database.
- Use a complete accepted route for an excursion and its continuation. Preserve authored waypoint
  provenance and on-route access anchors. Keep Recorder independent.
- Offer explicit recovery of an accepted Assistant journey. Do not add automatic general navigation
  resume. Reuse the Metadata object while preserving RetentionMachine's archive/retention authority.
- Easier route remains unavailable during a visit. A route with an unresolved accepted blockage also
  refuses Assistant replanning. The existing detour entry remains available outside these constraints.
- Use deterministic source/category/text/image rules, large ordered-dither photos and selected-item
  Sources. Missing or unsupported content stays explicit. No AI generation or per-site curation.
- Keep search thresholds and work caps in named tunable policies. These are initial implementation
  choices, not measured universal rankings. More places stays accessible beyond the shortlist.

The public [design study](../../../content/software/ride-assistant-study.md) and accepted capture
collections describe the layouts. The epic and linked children define the production work.

## Planning-change validation

Passed `python3 docs/build_docs.py --check-links`, `python3 tools/suite_registry.py check` in the
prototype worktree, and `git diff --check`. The registry check found 66 suites and 316 execution
units. An initial `obc suites check` used the main checkout instead; the direct repository command
above is the relevant result for this worktree.

Verified the dependency graph has no cycle; all 13 GitHub bodies/titles match their local files;
all 13 native sub-issues have parent #1734; the published parent body matches `epic.md`. Child
publication changes only dependency links from the reviewed hashes. The issue map records both
reviewed and published hashes. No Rust tests, simulator build, UI sweep, resource build or device
flash was run for this plan-only change.
