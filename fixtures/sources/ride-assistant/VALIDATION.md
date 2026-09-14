# Initial source validation

Source implementation: `90373c16`; stronger source identity checks: `3b7bf18b`.
All four initial source archives were uploaded with `tools/obc fixtures publish` and verified
through the public fixture domain. The catalog records the exact source archive hashes.

Checks:

- `cargo build --release -p obc-pack -p obc-dem`: passed.
- `python -m unittest discover -s tools/tests -v`: 173 tests passed. The interpreter used the
  repository's `tools/requirements-test.txt` in a local virtual environment. The first attempt
  lacked `xmlrunner`; installing the declared dependency resolved that environment failure.
- `tools/obc suites check`: passed (71 suites and 354 discovered execution units).
- `OBC_FIXTURE_CACHE="$PWD/fixtures/build/clean-cache" tools/obc fixtures sync assistant-inputs`:
  fetched all four source packages from the public store. A disk-full interruption occurred
  after OSM and terrain; it did not create a ready Wiki package. Sync resumed after disposal
  of published acquisition duplicates and repository-managed stale build cleanup.
- The same cache with `tools/obc fixtures verify assistant-inputs` and
  `python3 fixtures/verify-assistant-inputs.py`: passed. It verifies all 11 OSM objects, four
  Wikidata revisions/coordinates, 11 rendered article revisions, and eight image candidates
  against Commons original SHA-1 values as well as the capture SHA-256 values.
- `python3 docs/build_docs.py --check-links`: passed.

Offline builds use the same verified cache and source commit `3b7bf18b`:

```sh
OBC_FIXTURE_CACHE="$PWD/fixtures/build/clean-cache" \
  bash fixtures/build-map-package.sh assistant all
OBC_FIXTURE_CACHE="$PWD/fixtures/build/clean-cache" \
  OBC_FIXTURE_BUILD_DIR="$PWD/fixtures/build/maps-second" \
  bash fixtures/build-map-package.sh assistant all
```

Both calls run the actual DEM baker and map packer. They do not contact source services.
The first writes `fixtures/build/maps`; the second writes `fixtures/build/maps-second`.
The two archives must match byte for byte before publication:

| Archive | Bytes | SHA-256 |
| --- | ---: | --- |
| `sim-assistant-meiringen` | 7,090,063 | `caae3c9e829aae67b63d0583493629da48c1280951178acc4ddb3045edb1ac75` |
| `sim-assistant-west-cork` | 1,627,912 | `6277d6dc3b510c92cba6983090c878c3185f394546b668984a628076afb091f0` |

No application source changed. Rust unit/clippy, simulator UI sweep, resource measurement,
wake-profile isolation and device tests were deliberately omitted for this source/recipe task.
The source verification tool was also exercised with a missing Wiki package: it returned an
actionable `run tools/obc fixtures sync assistant-inputs` error without starting a build.

Acceptance still pending in dependent issues: integrated landmark map content, country-wide
content selection/count, real visit and easier-route production behavior, deterministic GUI
clock/offset plumbing, and hardware tests. Initial offline maps and GPS motion do not prove
those features work. No physical device is needed to continue the independent work.
