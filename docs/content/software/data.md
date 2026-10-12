---
title: Data operations
description: Targets that serve releases, and updates that keep what did not change.
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

A layer is stale for exactly three causes. Its version changed. A source has a newer version to
use. Or an input changed, such as a layer or a source that it reads, or its options. Status gives
one cause per layer, such as `format 19 → 20: readers need new data` or `follows device/map`. To
see a code change without a version change, name the layer:
`obc data update local --only device/terrain` builds it again. The layers that read it build again
only when its bytes change.

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
unresolved work and blocks the update. This distinction prevents server pressure from appearing
as missing content. The [shared content contract](src:specs/wikimedia-content.md) defines
identity, retained inputs and selection outcomes.

## Targets and releases

A target is a place that serves data: the public bucket, or a folder on one machine. A target
serves one release. The release records its settings, every output with the source versions that
it read, and the pointers that clients read first. So a target needs no other state, and any
machine can read what it serves. The [target contract](src:specs/obc-data.md) defines the files.

An update compares the steps of now with the serving release. It bakes only the stale layers and
keeps every other output by reference. A kept output is read from the files of the release, not
from the local store, so two machines that update the same target agree on the bytes. The
settings stay in the release: the next update keeps the region, the layers and the policies
that the last one used.

The switch is one conditional write of `release.json`, before the pointers change. The release
that served becomes the rollback. A stopped update leaves the target on its serving release, and
the next update reuses what it baked. Files of the older publishers are never written or removed.

Use the [data README](src:host/obc-data/README.md) for setup, credentials and daily commands.
