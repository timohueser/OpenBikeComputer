# Capture geographic landmark sources

A one-shot acquisition command that writes country-scale raw sources to a local map-baker source
directory. **Do not publish that directory to the dev fixture bucket or the production maps
bucket**, and do not register it as a fixture; see
[the fixture package workflow](../../README.md#add-or-replace-data) for what does belong there.
The compiled landmark content travels in ordinary maps, so no simulator or device ever needs the
raw archive.

## Acquire

Build the compiler and get the pinned country boundary first. Run from the repository root:

```sh
cargo build -p obc-bake --locked
tools/obc fixtures sync assistant-osm
OBC_LANDMARK_CAPTURE="${OBC_LANDMARK_CAPTURE:-$HOME/.cache/openbikecomputer/bake/landmarks/switzerland}"
python3 tools/landmark_capture.py \
  --boundary "$HOME/.cache/openbikecomputer/fixtures/by-id/assistant-osm/switzerland-boundary.geojson" \
  --policy host/obc-pack/src/landmarks/policy.json \
  --select-with target/debug/obc-bake \
  --out "$OBC_LANDMARK_CAPTURE"
```

The command uses two workers and at most four requests per second in total. It queries each
included category root inside the boundary's bounding box, deduplicates QIDs, then captures the
raw entities, their P279 class closure, the en/de/fr/es articles at exact revisions, and the P18
and lead-image candidates with their Commons metadata. Responses land in `queries/`, `entities/`,
`classes/`, `locales/`, `articles/` and `images/`, with `manifest.json` recording every URL, byte
count, SHA-256 and outcome. The response limit is 32 MiB.

`obc-bake landmark-content` makes every text, image and attribution decision afterwards, offline. Site
eligibility comes from the exact polygon and the category policy, not from P17 country claims or a
curated list, and the acquisition checks its category file digest against the compiler's embedded
policy digest so the two cannot disagree.

**Resume with the same arguments and the same output directory.** A completed request is reused
only after its URL, byte count and digest check out. Failed requests stay failed; `--retry-failed`
makes one more attempt at each, and the earlier records stay in `attempts/`. A changed boundary,
category policy, language set or locale policy needs a **new** output directory. Never run two
capture processes against one output directory.

An incomplete acquisition exits with status 2 and keeps the bytes it captured. Unresolved root
queries, source requests or class closure make country coverage incomplete: do not report their
absence as zero landmarks or zero images.

Keep a separate copy of the built `obc-bake` when another build can replace the target binary; the
capture checks its digest before and after selection.

## Compile

Check offline reproducibility by compiling twice into empty directories:

```sh
target/debug/obc-bake landmark-content --snapshot "$OBC_LANDMARK_CAPTURE/manifest.json" \
  --boundary "$OBC_LANDMARK_CAPTURE/boundary.geojson" --out .artifacts/content-a
target/debug/obc-bake landmark-content --snapshot "$OBC_LANDMARK_CAPTURE/manifest.json" \
  --boundary "$OBC_LANDMARK_CAPTURE/boundary.geojson" --out .artifacts/content-b
diff -r .artifacts/content-a .artifacts/content-b
```

For the optimized count over a whole country, build `--release` and use a fresh output directory
for each pass:

```sh
cargo build -p obc-bake --release --locked
target/release/obc-bake landmark-content \
  --snapshot "$OBC_LANDMARK_CAPTURE/manifest.json" \
  --boundary "$OBC_LANDMARK_CAPTURE/boundary.geojson" \
  --out .artifacts/switzerland-content
```

Acquisition counts are not usable-content counts: the compiler applies its text, licence and
attribution limits afterwards. [`switzerland-recount.json`](switzerland-recount.json) pins one
such run by source manifest, boundary, compiler, policy and output hashes.

## Retention

Keep the capture locally while the country build and its validation need it. Keep the source
hashes, the recipe, the compiler identity and the measured counts in Git, and remove the local raw
capture when that work is done. A fresh acquisition sees new upstream data; it cannot reproduce an
old capture from its hashes alone.
