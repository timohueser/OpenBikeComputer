---
title: Data operations
description: Saved inputs, exact plans, retained work and one publication owner.
copy: ai
---

# Data operations

The device and planner consume built data, not a live source query. A region chooses the
coverage. Product options choose what to build. Source versions choose the input bytes.
Keeping these choices explicit makes a result repeatable and lets unchanged work be reused.
The [data contract](src:specs/obc-data.md) defines these boundaries.

## One plan before work

Live and Local use saved configuration. The interface writes region and source-policy changes
as ordinary files, so their Git review remains separate from a data apply. Local has its own
region and layers. Its saved releases retain source versions until an explicit Live refresh.
An upstream check does not silently replace those pins.

A plan separates required work from optional source moves. Changed geometry, product options
or producer code can require work even when no source moves. Required rows cannot be unchecked.
A source move is deliberate, and a source with several requests must expose each request's
availability. This avoids claiming that one cached version covers every area.
An incomplete preview needs input preparation before it can become an exact review.

Apply uses the reviewed plan. If its inputs or configuration change, the operator must review
again. Preparation and build can return usable work without publishing it. Even a no-change
Live apply verifies the selected data and asks for consent before it records automatic approval.
See [publication and approval](src:specs/obc-data.md).

## Retained work and one owner

The selected machine builds and verifies. Its admitted worker survives a closed terminal.
The viewer can hide, reopen or observe that Run without changing it. Stop drains local work;
it does not kill an admitted publication after handoff.

One configured VPS owner changes published pointers. A durable intent prevents a competing
commit from guessing whether an interrupted write completed. Observing an unknown owner result
keeps it unknown. Explicit reconciliation records only a verified final reply.
This keeps publication separate from the lifetime of a laptop connection.

Publication and automatic approval have separate outcomes. A result can publish data while
approval remains unavailable. An enabled schedule can also be blocked by its host setup or
approval. Disabling future handoff does not discard work already building.
The [automatic-work contract](src:specs/obc-data.md) defines admission.

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
