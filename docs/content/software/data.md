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

## Layer versions, not code

Each layer, such as the device map, has two numbers. Its format is the number that its readers
check, so a reader that needs new data makes the layer stale with no other edit. Its revision
changes by hand in the change that improves the output. A refactor changes neither number, so it
rebuilds nothing. A rebuild that gives the same bytes stops there: the layers that read it keep
their keys. The [key contract](src:specs/obc-data.md) defines what a key holds.

## Shared landmark and peak content

Landmarks and peaks use the same acquired Wikimedia facts and selection policy. Exact OSM
links identify subjects. A subject can serve several regions without another download.
Source revisions identify acquired bytes; a new layer revision rebuilds outputs from retained inputs.
Pinned compilation works offline. Freshness checks remain separate network work.

The device keeps short articles, photos and credits in its map. Planner detail views keep
articles and source links, then load credited photos online. Routing packages carry photo
references without image payloads. An unavailable photo leaves useful text available.
Landmarks are selectable route stops. Peaks keep their summit coordinates and separate device collection.

An absent optional article or photo does not block a map. A failed source request remains
unresolved work and blocks preparation. This distinction prevents server pressure from appearing
as missing content. The [shared content contract](src:specs/wikimedia-content.md) defines
identity, retained inputs and selection outcomes.

## One plan before work

Live settings stay in the data store. A selection copies its full region definition, so a
preset edit cannot silently change coverage. Apply records the reviewed settings in each
release. Undo restores the applied region, layers and policies. Local has its own region and layers.

A plan separates required work from optional source moves. Changed geometry, product options
or a layer version can require work even when no source moves.
A source move is deliberate, and a source with several requests must expose each request's
availability. This avoids claiming that one cached version covers every area.
An incomplete preview needs input preparation before it can become an exact review.

Apply uses the reviewed plan. If its inputs or configuration change, the operator must review
again. Preparation and build can return usable work without publishing it. Even a no-change
Live apply verifies the selected data and asks for consent.

## Retained work and a direct apply

The selected machine builds and verifies. Its started run survives a closed terminal.
`obc data runs RUN --follow` does not change the Run. Stop drains the current work.

The machine that applies also publishes. It uploads only what R2 lacks, so a repeated apply
continues where a stopped one ended. Clients read only the pointer, and the apply writes the
pointers last, after one more check that no pointer changed since the review. Until then, live
does not change.

Old data stays until it is replaced. An apply removes only the files of earlier releases of this
tool that the reviewed plan listed, after clients that read an old pointer had time to finish.
Files of the older publishers are never removed. The [apply contract](src:specs/obc-data.md)
defines the order.

Use the [data README](src:host/obc-data/README.md) for setup, credentials and daily commands.
