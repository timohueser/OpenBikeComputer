# Fixtures

The registry of large development test data. [`catalog.toml`](catalog.toml) is the authority for
what fixture data exists, which files form a runnable scenario, and how to get it. Consumers
must not know bucket paths or reach into another crate's `assets/` directory. The catalog is
tracked. Large generated and captured bytes are not: they are packages in a bucket, addressed by
SHA-256.

Not here: small format vectors and goldens (beside their tests, and `specs/vectors/`), the demo
map the apps ship (`sim/desktop/assets/`), and the production map-baker cache.

## Where to start

| Path | What |
| --- | --- |
| `catalog.toml` | Packages (`archive`, `sha256`, `bytes`, `provenance`, `license`, `tracked_sources`), scenarios (which files form a simulator run) and profiles (`sim`, `test`, ...). |
| `tools/fixtures.py` | The registry: catalog validation, the cache, `pack`, `publish`, `prune`. Python standard library only. |
| `host/obc-fixtures` | The one path convention that Rust tests use: `file()`, `read()`, `file_in()`. |
| `tools/check_fixture_policy.py` | The guard: the archive budget, and no large blob in Git. |
| `build-map-package.sh`, `build-assistant-package.py` | Build the registered map packages. |
| `generate_peak_view.py` | Bakes the `sim-peak-view` terrain (needs NumPy and Pillow). |
| `verify-*.py` | Offline checks of captured sources; manual suites. |
| [`sources/`](sources/README.md) | Small authored inputs that stay in Git, one directory per package family. |

## Run

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
obc test fixtures -p CRATE        # sync the test profile, then run the tests that read packages
```

The cache defaults to `~/.cache/openbikecomputer/fixtures`; `OBC_FIXTURE_CACHE` relocates it.
Packages are addressed by SHA-256, and `by-id/` gives tests stable logical paths. Downloads and
extraction are atomic, and archives, manifests and every extracted file are verified. The cache
keeps the archive and the extracted tree, so it needs about twice the download size.

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
holds the source areas, reads the canonical boxes from `data/regions/`, and writes under
`fixtures/build/`. Build Grimsel terrain before its map, or use the `all` target.

## Gotchas

- The bytes of a package fix three catalog values together: `sha256`, `bytes`, and `archive`,
  which must be `packages/<sha256>.tar.gz`. One immutable package has one identity, so a
  published revision can never be overwritten.
- A Rust test that reads a package needs `required-features = ["external-fixtures"]`, and the
  crate must declare the feature. Without the gate the test lands in the fast tier, which runs
  with no sync, and fails there. `tools/test_plan.py` splits the tiers on this feature.
- `obc_fixtures::file` and `read` assume the `test` profile. A package that a CI test reads must
  be in `[profiles.test]`, directly or through a scenario. For another profile use `file_in`.
- Tests never download. A missing package panics with the `obc fixtures sync` command to run.
- No CI step checks `tracked_sources`. Only `obc fixtures verify` compares a tracked source with
  its package, and a `sync` re-extracts on a mismatch without a failure. After you change a
  tracked source or pack a package again, run `obc fixtures verify` yourself.
- Never take an extract box from a map header: the header box is wider than the extract, so the
  box grows on every repack.
- The `grimsel-demo` target of `build-map-package.sh` is not a fixture. It writes the shipped
  asset `sim/desktop/assets/grimsel-demo.obcm`.
- A local Peak View bake does not replace the registered package; the simulator reads the cache
  unless `OBC_PEAK_TERRAIN_DIR` is set. After `generate_peak_view.py` writes `catalog.rs`, run
  `cargo fmt --all`.
- Any change under `fixtures/` selects every suite that declares fixtures, and the `test`
  profile is a download of about 90 MB.
- `pack` refuses symlinks and empty trees, and `verify` refuses extra or missing files.
- [`tools/fixture_catalog.py`](../tools/fixture_catalog.py) has nothing to do with this
  catalog. It serves a device-map catalogue to the browser tests of `builder/web`.

## What belongs where

| Kind of data | Where it lives |
| --- | --- |
| Small authored format vectors, parser corpora, pixel goldens | Beside their owning tests, in Git |
| Product demo assets the app ships | With that app |
| Shared authored routes, trips and replays | `fixtures/sources/`, and packed into scenarios |
| Large maps, terrain, provider captures, ride bundles | The dev fixture bucket |
| Country-scale raw landmark captures | The map-baker's local source cache only |

**Do not register or publish a country capture as a development fixture**, in any profile. The
1 GiB archive limit is a backstop, not a target: use the smallest input that covers the scenario.
Generated design-review screenshots belong in a pull request, not in a runtime asset folder.

## Package provenance

Every registered map is an OBCM v19 file with Light and Dark style tables. The Dark table holds the
authored Dusk skin. The geographic payload comes from a canonical bbox that is never self-sourced
from a header. Each one has a build record in `sources/ride-assistant/` that pins its source and
output digests.

| Package | Source | Build record |
| --- | --- | --- |
| `sim-grimsel` | Pinned `assistant-osm` Switzerland snapshot, on the canonical fixture bbox. OBCT terrain from Copernicus GLO-30 tile `N46_00_E008_00`. OBCR v5 route. Landmark text, photos and credits from the pinned `assistant-switzerland-content` package. | [grimsel-v19.json](sources/ride-assistant/grimsel-v19.json) |
| `sim-monaco` | Pinned `assistant-osm` Monaco snapshot, plus the project-authored up-ahead GPX. | [monaco-v19.json](sources/ride-assistant/monaco-v19.json) |
| `sim-freiburg` | Geofabrik `europe/germany/baden-wuerttemberg/freiburg-regbez`, box `7.77,47.97,7.93,48.14`. 12 by 19 km of the Rhine plain with one city, three towns, 26 villages and 14 hamlets. No terrain, no route, no track: it exists for the settlement labels. | [freiburg-v19.json](sources/ride-assistant/freiburg-v19.json) |
| `sim-assistant-west-cork` | A complete-relation extract of the pinned Ireland snapshot, with compiled landmark content. | [west-cork-v19.json](sources/ride-assistant/west-cork-v19.json) |
| `sim-assistant-meiringen` | A crop of the pinned Swiss national PBF, with compiled landmark and peak content. Not a full-country map. | [meiringen-v19.json](sources/ride-assistant/meiringen-v19.json) |

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
`tracked_sources`: after a changed skyline, pack the package again and run `obc fixtures verify`.

## Peak article evidence

The `peak-articles` profile holds one bounded raw source package and one compiled catalogue.
`peak-wiki` keeps nine Swiss OSM summit nodes and the source responses for seven linked articles;
`peak-content` holds the compiled articles, language variants, photos and summit associations.
Peak articles use a separate map section from landmarks, and the baker joins them to emitted
summit records by original OSM node ID — never by name or coordinate.

```sh
obc fixtures sync peak-articles
python3 fixtures/verify-peak-content.py
```

The verifier rebuilds discovery and compares two offline compilations.
[The source record](sources/peak-view/peak-articles-source.json) and
[the output record](sources/peak-view/peak-articles-content.json) pin the identities.

## Storage

`fixtures.openbikecomputer.com` is a read-only custom domain for the EU-jurisdiction R2 bucket
`obc-dev-fixture`. Developers and CI need no cloud credentials. Upload credentials are
maintainer-only and are not shared with the production map publisher.

The publisher reads `OBC_FIXTURE_R2_BUCKET`, `OBC_FIXTURE_R2_ENDPOINT`,
`OBC_FIXTURE_R2_ACCESS_KEY_ID` and `OBC_FIXTURE_R2_SECRET_ACCESS_KEY` from
`~/.config/openbikecomputer/credentials.env`. **The endpoint must include `.eu.` for this bucket**;
an endpoint built from an account ID alone is not the EU one.

This bucket is not a country source archive and not the production map-baker cache. Compiled
landmark text, photos and Sources travel in ordinary maps through the production map publication
path. **Raw source captures must never be moved into the production maps bucket.**
