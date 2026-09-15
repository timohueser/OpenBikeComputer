# Useful Easier route on real Swiss data

This case uses the existing authored `meiringen-loop-crossing.gpx` and the published Swiss
regional map. All 307 coordinates are unchanged. The production `obc-elevation` reader supplies
an elevation for every point from the map's native OBCT region: 593–611 m. No missing value is
filled. Normal GPX import produces a 6,526 m route with 56 m ascent and complete elevation.
The imported route has no authored mandatory waypoint. Its surface attribution remains unknown.

At source point 76 (265 seconds), the rider is at 46.7231259 N, 8.1945506 E. The matched route has
5,492 m and 44 m ascent remaining. The normal Easier planner finds a 1,034 m alternative with
4 m ascent and complete elevation. The 4,458 m saving meets the existing Shorter threshold;
the new route also satisfies its ascent allowance. The map reports 21 m rough surface on the
alternative. The current route's rough distance stays unknown.

[Choice](choice.png) → [review](review.png) → [accepted guidance](accepted.png).

The selected alternative rebuilt with the same payload CRC and exact costs before acceptance.
The final persistent-card remount verified the exact accepted and original payload fingerprints:
[card proof](card-proof.txt). The checkpoint is `Following`, has a 1,034 m upper bound, and retains
the original route. Candidate CRCs can differ between fresh cards because route attribution
includes the map's store identity; equality is checked within the actual card transaction.

## Reproduce through ordinary controls

Use the integrated simulator with fix `d613f826` or its cherry-pick. Load the approved
`sim-assistant-meiringen` archive `96cb46be…fd5ce53c`; the map must have SHA-256
`ebe53f369a558e2c4e8da593b05433e55f730d83d07a928d46f7a236ec145bba`.
`SIM` names that simulator executable, `MAP` names the map, and `OUT` is a new output directory.
Run from the repository root:

```sh
DATA=docs/assets/ride-assistant/implementation/ra13-easier
mkdir -p "$OUT"
"$SIM" --import "$DATA/meiringen-loop-dem.gpx" --routes-dir "$OUT/routes"
"$SIM" "$MAP" --routes-dir "$OUT/routes" --create-card "$OUT/easier.card"
PREP="p p p p b T A d d p"
for i in $(seq 1 30); do PREP="$PREP f"; done
"$SIM" --card "$OUT/easier.card" --gpx "$DATA/meiringen-loop-from-265.gpx" \
  --at 0 --boot --clock 2026-09-14T10:00 --utc-offset-min 120 \
  --script "$PREP p f p f" --expect-screen Map --png "$OUT/accepted.png"
```

The gestures start the imported ride, open the normal drawer and Assistant, choose Easier,
wait for bounded trials, open the review, and explicitly select Use this route. No route answer,
acceptance, arrival or cost is injected. Omit the final `p f p f` for the choice frame; append only
`p f` for the review frame. Use a separate new card for each capture.

The motion GPX is the suffix beginning at source point 76. The full imported route is retained.
This makes `--at 0` supply the same real position during and after the script. A probe using the
full replay with `--at 265` became stale when the headless post-script replay restarted at zero.
That failed probe is not acceptance evidence. The original successful save uncovered Assistant;
fix `d613f826` now returns directly to Map after the authoritative acceptance acknowledgement.

## Source derivation and verification

[Provenance](provenance.json) records exact map, original GPX and derived GPX hashes.
[The sampler](sample_dem.rs) reads the embedded terrain window with `TerrainElevation<4>`.
[The derivation script](enrich.py) adds those real samples and writes the motion suffix; it does
not alter source coordinates or timestamps. Compile the sampler against the same checkout's
`obc-elevation` and `obc-formats` libraries, then run:

```sh
python3 "$DATA/enrich.py" SAMPLER "$MAP" \
  fixtures/sources/ride-assistant/replays/meiringen-loop-crossing.gpx "$OUT/derived"
```

[The card inspector](card_proof.rs), linked against `obc-host-core`, uses the normal card mount,
route catalog and checkpoint APIs. It asserts that both persisted fingerprints match actual
route payloads. It does not edit the checkpoint or accept a route.

The actual final capture ran with network denied. A separately named simulator executable was
built from `d613f826` to preserve the shared simulator binaries. The App suite (901 library tests
and all integration suites), scoped App Clippy, registry, formatting and documentation checks
passed. The existing uncertain-acceptance test now also verifies that committed recovery returns
to Map and removes the Assistant parent. No UI sweep, firmware image, map rebake, resource
measurement or hardware test was run. This case establishes a useful real alternative and durable
acceptance; full integrated recorder continuity, longer motion and hardware acceptance remain
separate checks.

The original GPX is authored motion under the repository license. Elevation derives from the
pinned Copernicus DEM in the map; its existing attribution applies. No raw country capture or
source image is included here. Logs and the sparse card are retained in the implementation
worktree under `.artifacts/easier/`.
