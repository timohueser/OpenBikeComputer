# Capture geographic landmark sources

A one-shot acquisition command that writes country-scale raw sources to a local map-baker source
directory. **Do not publish that directory to the dev fixture bucket or the production maps
bucket**, and do not register it as a fixture; see
[the fixture package workflow](../../README.md#add-or-replace-data) for what does belong there.
The compiled landmark content travels in ordinary maps, so no simulator or device ever needs the
raw archive.

## Acquire

Build the compiler, get the pinned country boundary and extract, then read the candidates from
that extract. Run from the repository root:

```sh
cargo build -p obc-bake --release --locked
tools/obc fixtures sync assistant-osm
FIXTURES="$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-osm"
OBC_LANDMARK_CAPTURE="${OBC_LANDMARK_CAPTURE:-$HOME/.cache/openbikecomputer/bake/landmarks/switzerland}"
target/release/obc-bake landmark-candidates \
  --osm "$FIXTURES/switzerland.osm.pbf" --out .artifacts/switzerland-candidates.json
python3 tools/landmark_capture.py \
  --boundary "$FIXTURES/switzerland-boundary.geojson" \
  --candidates .artifacts/switzerland-candidates.json \
  --policy host/obc-pack/src/landmarks/policy.json \
  --select-with target/release/obc-bake \
  --out "$OBC_LANDMARK_CAPTURE"
```

Candidates are the QIDs the extract's own objects carry in a `wikidata` tag. Nothing is matched by
name or by coordinate, so discovery is offline and every landmark has a map object with an
approach. A tag that is not a QID is listed in `rejected` and does not stop the run. `--sweep` adds a Wikidata class query for the natural-curiosities group, whose places are
often unmapped; it is off by default.

The command uses two workers and at most ten requests per second in total. It sends `maxlag=5` and
a descriptive User-Agent, and it waits for the `Retry-After` of a `429` or a `503` before it asks
again. It captures the entities fifty per request, their P279 class closure, the en/de/fr/es
articles at exact revisions, and the P18 and lead-image candidates with their Commons metadata. A
batch whose response is larger than the compiler reads is asked for again in halves.
Responses land in `entities/`, `classes/`, `locales/`, `articles/`, `images/` and, with `--sweep`,
`queries/`, with `manifest.json` recording every URL, byte count, SHA-256 and outcome. The response
limit is 32 MiB.

`obc-bake landmark-content` makes every text, image and attribution decision afterwards, offline. Site
eligibility comes from the exact polygon and the category policy, not from P17 country claims or a
curated list, and the acquisition checks its category file digest against the compiler's embedded
policy digest so the two cannot disagree.

**Resume with the same arguments and the same output directory.** A completed request is reused
only after its URL, byte count and digest check out. Failed requests stay failed; `--retry-failed`
makes one more attempt at each, and the earlier records stay in `attempts/`. A changed candidate
list, boundary, category policy, language set or locale policy needs a **new** output directory.
Never run two capture processes against one output directory.

Manifest schema 2 records the request identity of every Commons category page. You can resume a
schema 1 capture in the same output directory to add missing continuation pages and write schema
2. The capture verifies and reuses each successful page. The compiler rejects all schema 1
category records because they cannot prove that every page was consumed.

An incomplete acquisition exits with status 2 and keeps the bytes it captured. Unresolved source
requests, sweep queries or class closure make country coverage incomplete: do not report their
absence as zero landmarks or zero images. A `wikidata` tag that names no item is a fact about the
extract, not a failure: `unresolved_identities` counts those and coverage stays complete.

Keep a separate copy of the built `obc-bake` when another build can replace the target binary; the
capture checks its digest before and after selection.

## Compile

Check offline reproducibility by compiling twice into empty directories:

```sh
target/release/obc-bake landmark-content --snapshot "$OBC_LANDMARK_CAPTURE/manifest.json" \
  --boundary "$OBC_LANDMARK_CAPTURE/boundary.geojson" --out .artifacts/content-a
target/release/obc-bake landmark-content --snapshot "$OBC_LANDMARK_CAPTURE/manifest.json" \
  --boundary "$OBC_LANDMARK_CAPTURE/boundary.geojson" --out .artifacts/content-b
diff -r .artifacts/content-a .artifacts/content-b
```

Acquisition counts are not usable-content counts: the compiler applies its text, licence and
attribution limits afterwards. [`switzerland-recount.json`](switzerland-recount.json) pins one
such run by source manifest, boundary, compiler, policy and output hashes.

## Retention

Keep the capture locally while the country build and its validation need it. Keep the source
hashes, the recipe, the compiler identity and the measured counts in Git, and remove the local raw
capture when that work is done. A fresh acquisition sees new upstream data; it cannot reproduce an
old capture from its hashes alone.
