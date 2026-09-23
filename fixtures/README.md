# Fixture registry

[`catalog.toml`](catalog.toml) is the authority for what fixture data exists, which files form a
runnable scenario, and how to get it. Consumers must not know bucket paths or reach into another
crate's `assets/` directory.

The catalog is tracked. Large generated and captured bytes are not.

## Daily use

```sh
obc fixtures list                 # scenarios, cache state, download size
obc fixtures show grimsel         # inputs, provenance, and packages
obc fixtures sync sim             # all simulator packages
obc fixtures sync test            # every external package used by tests
obc fixtures verify test          # re-hash archives and every extracted file
obc fixtures prune                # dry-run stale cache cleanup
obc fixtures prune --apply

obc sim grimsel                   # sync on first use, then run
obc sim monaco-upahead
```

The cache defaults to `~/.cache/openbikecomputer/fixtures`; `OBC_FIXTURE_CACHE` relocates it.
Packages are addressed by SHA-256, and `by-id/` gives tests stable logical paths. Downloads and
extraction are atomic, and archives, manifests and every extracted file are verified.
`tracked_sources` mappings make CI fail if a small authored source changes without its external
package being rebuilt.

Python 3.11 or newer. No Python package dependencies.

## Add or replace data

1. Make a clean staging directory holding only the package's final layout.
2. `obc fixtures pack ID DIR --output ID.tar.gz`. Packing is deterministic.
3. Add or update the package entry with the printed byte count and SHA-256, then compose it into
   scenarios and profiles. Record source date, geography, transformation and licence in the
   catalog. Each archive is at most 1 GiB.
4. `obc fixtures publish ID ARCHIVE`. The maintainer-only command refuses bytes that differ from
   the catalog, uploads immutably to R2, and verifies the object through the public domain.
5. Run the registry tests, and `obc fixtures sync` and `verify` from an empty cache.

`fixtures/build-map-package.sh` is the only supported way to build the registered map packages. It
holds the canonical bboxes and source URLs and writes under `fixtures/build/`. Build Grimsel
terrain before its map, or use the `all` target.

## What belongs where

| Kind of data | Where it lives |
| --- | --- |
| Small authored format vectors, parser corpora, pixel goldens | Beside their owning tests, in Git |
| Product demo assets the app ships | With that app |
| Shared authored routes, trips and replays | `fixtures/sources/`, and packed into scenarios |
| Large maps, terrain, provider captures, ride bundles | The dev fixture bucket |
| Country-scale raw landmark captures | The map-baker's local source cache only |

**Do not register or publish a country capture as a development fixture**, in any profile. The
1 GiB archive limit is a backstop, not
a target: use the smallest input that covers the scenario. Generated design-review screenshots
belong in a pull request, not in a runtime asset folder.

## Package provenance

Every registered map is an OBCM v18 file built by `fixtures/build-map-package.sh` on a canonical
bbox that is never self-sourced from a header. Each one has a build record in
`sources/ride-assistant/` that pins its source and output digests.

| Package | Source | Build record |
| --- | --- | --- |
| `sim-grimsel` | Pinned `assistant-osm` Switzerland snapshot, on the canonical fixture bbox. OBCT terrain from Copernicus GLO-30 tile `N46_00_E008_00`. OBCR v5 route. Landmark text, photos and credits from the pinned `assistant-switzerland-content` package. | [grimsel-v18.json](sources/ride-assistant/grimsel-v18.json) |
| `sim-monaco` | Pinned `assistant-osm` Monaco snapshot, plus the project-authored up-ahead GPX. | [monaco-v18.json](sources/ride-assistant/monaco-v18.json) |
| `sim-freiburg` | Geofabrik `europe/germany/baden-wuerttemberg/freiburg-regbez`, box `7.77,47.97,7.93,48.14`. 12 by 19 km of the Rhine plain with one city, three towns, 26 villages and 14 hamlets. No terrain, no route, no track: it exists for the settlement labels. | [freiburg-v18.json](sources/ride-assistant/freiburg-v18.json) |
| `sim-assistant-west-cork` | A complete-relation extract of the pinned Ireland snapshot, with compiled landmark content. | [west-cork-v18.json](sources/ride-assistant/west-cork-v18.json) |
| `sim-assistant-meiringen` | A crop of the pinned Swiss national PBF, with compiled landmark and peak content. Not a full-country map. | [meiringen-v18.json](sources/ride-assistant/meiringen-v18.json) |

When you run the fixture baker, set `OBC_GRIMSEL_LANDMARKS` to the content package's
`content.json` and `OBC_GRIMSEL_PEAKS` to the pinned `peak-content/peaks.json`.

## Ride Assistant inputs

The `assistant-inputs` profile holds independent OSM, terrain, Wiki and authored replay packages.
[The source recipes](sources/ride-assistant/README.md) have the exact sources, licences, offline
build commands and scenario clocks. The four-site Wiki input is a review sample, not coverage.

## Peak View photographs

`peak-view-photos` is the only ground truth Peak View has: six photographs the owner took near
Engelberg, the skyline read off each one, and the terrain shard they were measured on.
`firmware/obc-app/tests/peak_view_photos.rs` draws the panorama at each position and holds the
root-mean-square difference to a recorded limit. [The package README](sources/peak-view/photos/README.md)
has the bake commands, digests and licences. The six `photos/<name>.json` files are
`tracked_sources`, so a changed skyline fails CI until the package is packed again.

## Peak article evidence

The `peak-articles` profile holds one bounded raw source package and one compiled catalogue.
`peak-wiki` keeps nine Swiss OSM summit nodes and the source responses for seven linked articles;
`peak-content` holds the compiled articles, language variants, photos and summit associations.
Peak articles use a separate map section from landmarks, and the baker joins them to emitted
summit records by original OSM node ID — never by name or coordinate.

```sh
tools/obc fixtures sync peak-articles
python3 fixtures/verify-peak-content.py
```

The verifier rebuilds discovery and compares two offline compilations.
[The source record](sources/peak-view/peak-articles-source.json) and
[the output record](sources/peak-view/peak-articles-content.json) pin the identities.

## Storage

`fixtures.openbikecomputer.com` is a read-only custom domain for the EU-jurisdiction R2 bucket
`obc-dev-fixture`. Developers and CI need no cloud credentials. Upload credentials are
maintainer-only and are deliberately not shared with the production map publisher.

The publisher reads `OBC_FIXTURE_R2_BUCKET`, `OBC_FIXTURE_R2_ENDPOINT`,
`OBC_FIXTURE_R2_ACCESS_KEY_ID` and `OBC_FIXTURE_R2_SECRET_ACCESS_KEY` from the gitignored
`tools/obc.local`. **The endpoint must include `.eu.` for this bucket.**

This bucket is not a country source archive and not the production map-baker cache. Compiled
landmark text, photos and Sources travel in ordinary maps through the production map publication
path. **Raw source captures must never be moved into the production maps bucket.**
