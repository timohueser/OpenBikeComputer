# Fixture registry

This directory is the one place that answers three questions: what fixture
data exists, which files form a runnable scenario, and how a developer obtains
it. The catalog is tracked; large generated and captured bytes are not.

## Mental model

The catalog at [`catalog.toml`](catalog.toml) is authoritative. Consumers must
not know bucket paths or reach into another crate's `assets/` directory.

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

The default cache is `~/.cache/openbikecomputer/fixtures`. Set
`OBC_FIXTURE_CACHE` to relocate it. Packages are addressed by SHA-256 and a
`by-id/` view gives tests stable logical paths. Downloads and extraction are
atomic; archives, manifests, and every extracted file are verified.
`tracked_sources` mappings additionally make CI fail if a small authored source
changes without rebuilding its external package.

Python 3.11 or newer is required (`tomllib` is part of the standard library).
There are no Python package dependencies.

## Adding or replacing data

1. Make a clean staging directory containing only the package's final layout.
2. Run `obc fixtures pack ID DIR --output ID.tar.gz`. Packing is deterministic.
3. Add or update the package entry with the printed byte count and SHA-256,
   then compose it into scenarios/profiles. Each archive must be at most 1 GiB.
   Record source date, geography, transformation, and license in the catalog.
4. Run `obc fixtures publish ID ARCHIVE`. The maintainer-only command refuses
   bytes that differ from the catalog, performs an immutable R2 upload, sets
   long-lived cache metadata, and verifies the object through the public domain.
5. Run the registry tests and `obc fixtures sync/verify` from an empty cache.

For the two initial map packages, `fixtures/build-map-package.sh` preserves the
canonical bboxes and source URLs and writes its staging trees/archives under
`fixtures/build/`. Build Grimsel terrain before its map, or use the `all` target.

## What remains in Git

Small authored format vectors, parser corpora, and pixel goldens stay beside
their owning tests because code review needs their exact byte changes. Product
demo assets stay with the app that ships them. Shared authored inputs live in
`fixtures/sources/` and are also packed into their scenarios. Large maps,
terrain, provider captures, and realistic ride bundles for bounded development scenarios belong
in the dev fixture bucket. Country-scale raw landmark captures belong in the map-baker's local
source cache. Do not register or publish them as development fixtures, including in opt-in profiles.
Do not split country captures into smaller packages to bypass this scope. The 1 GiB archive limit
is a backstop, not a target size; use the smallest input that covers the scenario.
Generated design-review screenshots belong in PRs or project documentation, not a runtime asset folder.

## Package provenance

- `sim-grimsel`: an **OBCM v18** file packed from the pinned `assistant-osm`
  Switzerland snapshot dated 2026-09-13, on the canonical fixture bbox; its OBCT terrain is derived from Copernicus GLO-30 tile
  `N46_00_E008_00` and is unchanged (OBCT is a separate format and did not move).
  The GPX/OBCR/OBT inputs are project-authored and byte-identical to their
  `tracked_sources` originals. The Grimsel route uses OBCR v4 with route facts. The map contains selected text, compressed photos, and credits from the pinned
  `assistant-switzerland-content` schema 2 package. It uses the optional landmark section.
  [The build record](sources/ride-assistant/grimsel-v18.json) pins source and output hashes.
  Set `OBC_GRIMSEL_LANDMARKS` to its `content.json` and `OBC_GRIMSEL_PEAKS` to the
  pinned `peak-content/peaks.json` when you run the fixture baker.
- `sim-monaco`: an **OBCM v18** file from the pinned `assistant-osm` Monaco
  snapshot dated 2026-09-13, on the canonical fixture bbox, plus the unchanged
  project-authored up-ahead GPX. [The build record](sources/ride-assistant/monaco-v18.json)
  pins its source and output identities.
- `sim-freiburg`: an **OBCM v18** file packed from the Geofabrik
  `europe/germany/baden-wuerttemberg/freiburg-regbez` snapshot dated 2026-08-03, on the
  canonical box `7.77,47.97,7.93,48.14`. It is 12 by 19 km of the Rhine plain, from the city of
  Freiburg north to Emmendingen, with the settlement density of a typical ride: one city, three
  towns, 26 villages and 14 hamlets, 16 of them with a population. It holds no terrain, no route
  and no track, because it exists for the settlement labels.
  [The build record](sources/ride-assistant/freiburg-v18.json) pins its source and output
  identities.

### Revision log: repacked at OBCM v18 (settlements, #1899)

Every registered map moved to a new immutable revision, because a v18 reader refuses a v17
file outright. All of them were packed again through the sanctioned path,
`fixtures/build-map-package.sh`, on the unchanged canonical boxes, from the same pinned
sources as the v17 revision. No box was self-sourced from a header.

| package | v17 archive | v18 archive | map file | v17 | v18 |
| --- | ---: | ---: | --- | ---: | ---: |
| `sim-grimsel` | 3 478 964 B | 4 137 061 B | `grimsel.obcm` | 3 971 792 B | 4 774 608 B |
| `sim-monaco` | 443 213 B | 456 152 B | `monaco.obcm` | 724 480 B | 742 400 B |
| `sim-assistant-meiringen` | 30 400 331 B | 33 484 275 B | `meiringen.obcm` | 44 881 840 B | 48 740 272 B |
| `sim-assistant-west-cork` | 2 426 077 B | 2 435 914 B | `west-cork.obcm` | 4 749 504 B | 4 783 296 B |
| `sim-freiburg` | — | 8 135 640 B | `freiburg.obcm` | — | 11 746 304 B |

**Settlements are the small part of that growth.** Grimsel packed from the same source with
the settlement rows removed gives 4 772 560 B, so its 19 settlements cost 2 048 B — four
512-byte chunks with their index. The other 800 768 B come from pull request #1893, which
changed `builder/presets/schema.json` and the packer's path and contour handling. These
fixtures were last packed before it, so the format bump is the first time that work reaches
them.

A settlement stores its OSM `short_name` when that name is shorter, because the map shows 12
characters: the Freiburg record reads `Freiburg`, and Meiringen's seven `Hasliberg …` and
`… bei Interlaken` records read their short names. Nine records over all the packages changed
name for this rule; every other settlement is unchanged.

Grimsel's OBCT sidecar is byte-identical again (786 560 B), and the shipped demo map keeps
the surface terrain it already carried, both verified by digest. The routing pin also holds:
all four stock profiles' frozen Innertkirchen→Grimsel digests
(`obc-route`'s `the_registered_grimsel_fixture_routes_byte_identically_on_every_profile`)
are unchanged across the format bump, the settlement capture and #1893's preset.

### Revision log: repacked at OBCM v14 (FS7.5b, #1420)

`sim-grimsel` and `sim-monaco` moved to a new immutable revision because a v14
reader refuses a v13 file outright. Both were regenerated through the sanctioned
path, `fixtures/build-map-package.sh`, on the canonical bboxes and the current
`builder/presets/schema.json`.

The pinned `2026-08-08` snapshots are no longer hosted — Geofabrik serves
roughly the last 90 days — so this repack necessarily used the current
`2026-08-17` one, and a fortnight of OSM edits rides along with the format bump.
That is a deliberate refresh, not a drift: the extract bboxes in
`build-map-package.sh` are unchanged and were **not** self-sourced from the old
headers.

Grimsel's OBCT sidecar is byte-identical (786 560 B) — terrain has its own
revision track and did not move. The maps did:

| file | v13 | v14 |
| --- | --- | --- |
| `grimsel.obcm` | 3 664 384 B | 3 804 160 B (+3.8%) |
| `monaco.obcm` | 710 144 B | 718 336 B (+1.2%) |

Most of that is v14's §1.2 unit filler — roughly half a percent of the geometry
chunk bytes plus a gap at each region boundary — with the content delta on top.

What did **not** move is the routing pin: all four stock profiles' frozen
Innertkirchen→Grimsel digests
(`obc-route`'s `the_registered_grimsel_fixture_routes_byte_identically_on_every_profile`)
still hold across both the format bump and the snapshot refresh, so the
addressing change is byte-neutral to the router and the fortnight of edits
missed that corridor.

## Ride Assistant inputs

The `assistant-inputs` profile contains independent OSM, terrain, Wiki, and authored replay
packages. See [the source recipes](sources/ride-assistant/README.md) for source dates, exact
revisions, licenses, offline build commands, review identities, and remaining simulator wiring.
The Swiss OSM input is country-wide. The separate compiled Swiss content package contains
1,478 sites, 2,391 article variants and 1,109 photos. The retained capture has English, German
and French output, with no Spanish article or locale entity capture. The four-site Wiki input
is only a review sample. Both West Cork and the Swiss regional simulator maps use OBCM v18 with native terrain and compiled landmark
content. The Swiss map is a crop, not a full-country map. See the source recipes for exact
coverage, scenario clocks and the persistent-card option.


## Peak View photographs

`peak-view-photos` is the only ground truth Peak View has: seven photographs the owner took near
Engelberg, the skyline read off each one, and the terrain shard the skylines were measured on.
`firmware/obc-app/tests/peak_view_photos.rs` draws the whole panorama at each recorded position
and holds the root-mean-square difference per view to a recorded limit.

[The package README](sources/peak-view/photos/README.md) is the tracked copy of the README inside
the package. It holds the bake commands and digests, how each skyline was read, and the two views
whose haze puts the read line on a near ridge rather than on the far horizon. The seven
`photos/<name>.json` files are `tracked_sources`, so a change to a recorded skyline fails CI until
the package is packed again. The photographs themselves are in the package only; the commit that
retired `scratch/peak-view/` is where they were before.

## Peak article evidence

The `peak-articles` profile contains one bounded raw source package and one compiled catalogue
for host map integration. `peak-wiki` retains nine original Swiss OSM summit nodes and the source
responses for seven explicitly linked articles. `peak-content` contains the seven compiled
articles, all usable UI language variants, six photos, and eight summit associations. Two Piz
Starlex nodes share one article and photo outcome. Pointe Kurz and Tourbillon use direct Wikipedia
tags. Gross Wendenstock has no link. The Piz Starlex photo is rejected because its attribution is
too long; its text remains available.

The compiled catalogue feeds the normal OBCM v18 map paths. Grimsel and the shipped demo
include Mönch. The Meiringen regional map includes Titlis, Eiger and Mönch. The baker joins
articles to emitted summit records by original OSM node ID. Gross Wendenstock remains a
summit without an article. Peak articles use a separate section from landmarks; original
article and photo credits remain in each collection. These bounded examples do not establish
complete regional or country article coverage.
[The source record](sources/peak-view/peak-articles-source.json) identifies the unchanged OSM node
slice and capture. [The output record](sources/peak-view/peak-articles-content.json) records the
compiler and catalogue digests. Run `tools/obc fixtures sync peak-articles`, then
`python3 fixtures/verify-peak-content.py` to rebuild discovery and compare two offline compilations.

## Storage contract

The dev fixture bucket holds test and simulator inputs. It is not a country source archive or
the production map-baker cache. Country acquisition retains raw responses locally for the build
and its validation. Keep the source hashes, recipe and count report in Git. Remove local raw
captures when that work is complete; retain them longer only for an active reproducibility need.
Compiled landmark text, photos and Sources belong in ordinary map content and use the production
map publication path. Raw source captures must not be moved into the production maps bucket.

`fixtures.openbikecomputer.com` is a read-only custom domain for a separate R2
EU-jurisdiction R2 bucket, `obc-dev-fixture`. Developers and CI need no cloud credentials. Upload credentials are
maintainer-only and are intentionally not shared with the production map
publisher. R2 Standard storage is appropriate because these packages are small,
occasionally replaced, and downloaded interactively.

The publisher reads `OBC_FIXTURE_R2_BUCKET`, `OBC_FIXTURE_R2_ENDPOINT`,
`OBC_FIXTURE_R2_ACCESS_KEY_ID`, and `OBC_FIXTURE_R2_SECRET_ACCESS_KEY` from the
gitignored `tools/obc.local`. The endpoint must include `.eu.` for this bucket.
