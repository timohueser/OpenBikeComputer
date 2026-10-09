---
title: Data operations
description: Saved inputs, exact plans, retained work and a direct Live apply.
copy: ai
---

# Data operations

The device and planner consume built data, not a live source query. A region chooses the
coverage. Product options choose what to build. Source versions choose the input bytes.
Keeping these choices explicit makes a result repeatable and lets unchanged work be reused.
The [data contract](src:specs/obc-data.md) defines these boundaries.

## Shared landmark and peak content

Landmarks and peaks share a manually prepared content version. An operator reads compressed
Wikidata and Wikipedia snapshots on a machine with enough disk space, prepares selected images
and publishes the result. Scheduled bakes select that version from R2. This lets road data
change often without repeated Wikimedia requests. Exact OSM links identify subjects across regions.

The device keeps short articles, photos and credits in its map. Planner detail views keep
articles and source links, then load credited photos online. Routing packages carry photo
references without image payloads. An unavailable photo leaves useful text available.
Landmarks are selectable route stops. Peaks keep their summit coordinates and separate device collection.

An absent optional article or photo does not block a map. A failed import or uncovered identity
remains unresolved work and blocks preparation. This prevents missing inputs from appearing
as absent content. The [shared content contract](src:specs/wikimedia-content.md) defines
identity, retained inputs and selection outcomes.

## One plan before work

Live settings stay in the data store. A selection copies its full region definition, so a
preset edit cannot silently change coverage. Apply records the reviewed settings in each
release. Undo restores the applied region, layers and policies. Local has its own region and layers. Its saved releases retain source versions until an explicit Live refresh.
An upstream check does not silently replace those pins.

A plan separates required work from optional source moves. Changed geometry, product options
or producer code can require work even when no source moves. Required rows cannot be unchecked.
A source move is deliberate, and a source with several requests must expose each request's
availability. This avoids claiming that one cached version covers every area.
An incomplete preview needs input preparation before it can become an exact review.

Apply uses the reviewed plan. If its inputs or configuration change, the operator must review
again. Preparation and build can return usable work without publishing it. Even a no-change
Live apply verifies the selected data and asks for consent.

## Retained work and a direct apply

The selected machine builds and verifies. Its admitted worker survives a closed terminal.
The viewer can hide, reopen or observe that Run without changing it. Stop drains the current
work and asks first.

The machine that applies also publishes. It uploads only what R2 lacks, so a repeated apply
continues where a stopped one ended. Clients read only the pointer, and the apply writes the
pointers last, after one more check that no pointer changed since the review. Until then, live
does not change.

Old data stays until it is replaced. An apply removes only the files of earlier releases of this
tool that the reviewed plan listed, after clients that read an old pointer had time to finish.
Files of the older publishers are never removed. The [apply contract](src:specs/obc-data.md)
defines the order.

## Local keeps original provenance

Local builds use current working-tree code and selected configuration. Compatible portable
layers reuse verified published bytes and retain the original producer identity. They do not
need the unused original compiler or a private intermediate archive. New work still requires
its actual tools. Native executables are built for the current host.

The saved Local release keeps useful bytes reachable by cleanup. Prepared apps share services
when they need the same files. A rebuild updates affected running services and leaves stopped
apps stopped. First entry is read-only; an explicit input check makes its network work visible.
See [portable data](src:specs/obc-data.md) and
[Local services](src:specs/obc-data.md).

Use the [data README](src:host/obc-data/README.md) for setup, credentials and daily commands.
