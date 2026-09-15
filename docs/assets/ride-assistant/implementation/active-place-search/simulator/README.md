# Offline simulator evidence at 9f6c93e6d

The five completed production flows and one loading frame passed with the real Swiss map and Meiringen loop. Each run starts from the same isolated card, opens the original route, starts recording, and settles normal GPS ticks at source progress 1,664 m. Network access is denied by sandbox-exec. Exact commands, input hashes, and the pinned binary hash are beside the captures.

The Water shortlist now contains arrivals at 17, 489, 970, and 972 m. The last choice replaces the earlier 3,739 m choice. Both runs use the same source map, route, progress, and four-nearby/four-corridor limit. The new corridor key follows the planner's selected future route occurrence and adds the straight-line connection estimate. Every admitted candidate is still fully routed before the measured ranking.

## Named captures

- [Water loading compass](active-water-finding.png)
- [Water choices](active-water-choices.png)
- [Water preview](active-water-preview.png)
- [Water preview after Back and Select](active-water-preview-reuse.png)
- [Resupply preview](active-resupply-preview.png)
- [Campsite preview](active-campsite-preview.png)

Water is category 0, Campsite is category 1, and Resupply is category 3. Each category starts from a fresh boot. Back and Select are used only within Water results to check preview reuse. No category index depends on a prior category cursor.

The loading pill uses the existing compass needle below its text. It turns by 120 degrees once per second. Its shared drawing and device repaint band is rows 88 through 143; this leaves the complete rider chevron clear. No elapsed-time cutoff is installed. The source test covers all three compass phases, their changed pixels, and the complete band bounds.

## Geometry and reuse

The read-only card inspector verifies payload CRCs and original object/revision identity. The retained candidates preserve the original prefix from the current progress and the complete remaining tail. Each display shape contains its stop and ends at its accepted rejoin. The original terminal vertex is preserved beyond the stored whole-metre total; the coalesced join is within 1.01 m. Full route and display-only CSV files are exported separately.

The nearest Water route has original anchors `[1664,1681,1681]` and accepted anchors `[0,17,18]`. The preview fits 18 m with five vertices; the stored route remains 4,863 m with 123 vertices. All geometry checks pass for Water, Resupply, and Campsite. The before/after Water cost comparison is in [water-choice-comparison.json](water-choice-comparison.json).

Water choices, preview, and Back/Select preserve identical route IDs, revisions, bytes, and card sequence 14. Both preview PNGs are byte-identical. See [reuse-and-geometry.json](reuse-and-geometry.json). All five completed frame hashes and geometry logs also match source 3faa84a65 exactly. These are persistence and visual checks; physical-device traces provide A* invocation and latency evidence.

## Verification

The whole App library and integration suites passed at compass source 4701a91e0, including all 918 library tests. Scoped App clippy with warnings denied passed at a0735206, before the constant-only band placement correction. The suite registry passed with 80 suites. Logs are [app.log](app.log) and [clippy.log](clippy.log). The release simulator was built from integrated head 9f6c93e6d and copied immediately to this directory. All six simulator exits and all five completed-route inspector exits are zero in [scenario-results.json](scenario-results.json).

No UI snapshot sweep, board build, or resource build was run by this agent. CI and the orchestrator own those gates. Simulator wall times are not device timings.

## Reproduce from a fresh card

Run from the repository root on macOS. Build `cargo build --release --locked -p obc-sim` at the recorded source head. Set `evidence` to this evidence directory. It contains the 1,366-byte original `meiringen-loop-dem.obcr` and `stationary.gpx`. Set `map` to the installed real `meiringen.obcm`; its required SHA-256 is `ebe53f369a558e2c4e8da593b05433e55f730d83d07a928d46f7a236ec145bba`. The local fixture cache path is `$HOME/.cache/openbikecomputer/fixtures/by-id/sim-assistant-meiringen/meiringen.obcm`. These runs need no network once this map is available.

```sh
evidence=docs/assets/ride-assistant/implementation/active-place-search/simulator
map="$HOME/.cache/openbikecomputer/fixtures/by-id/sim-assistant-meiringen/meiringen.obcm"
run_dir="$(mktemp -d /tmp/obc-find-XXXXXX)"
mkdir "$run_dir/routes" "$run_dir/exports"
cp "$evidence/meiringen-loop-dem.obcr" "$run_dir/routes/"
/usr/bin/sandbox-exec -p '(version 1)(allow default)(deny network*)'   target/release/obc-sim "$map" --create-card "$run_dir/base.obc" --routes-dir "$run_dir/routes"
```

Use a fresh copy of `base.obc` for each category. The following is the complete Water preview sequence. The five `T` GPS ticks, with normal `w` frames between them, select the current route occurrence at 1,664 m. The `A` input is the production Up+Select hold gesture. The 64 `f` frames complete sequential Find planning.

```sh
cp -c "$run_dir/base.obc" "$run_dir/water.obc"
script="p p p p T w T w T w T w T A p p $(python3 -c 'print(" ".join(["f"] * 64))') p f f"
/usr/bin/sandbox-exec -p '(version 1)(allow default)(deny network*)'   target/release/obc-sim --card "$run_dir/water.obc" --boot   --tracks-dir "$run_dir/exports" --gpx "$evidence/stationary.gpx"   --script-at 0 --at 0 --clock 2026-09-15T10:00 --utc-offset-min 120   --script "$script" --expect-screen VisitReview --png "$run_dir/water-preview.png"
```

For the loading compass frame, use only `p p p p T w T w T w T w T A p p` and expect `FindPlace`. For Water choices, omit the final `p f f` and expect `FindPlace`. For Water Back/reselect, append `b p f f` and expect `VisitReview`. For Campsite, replace `A p p` with `A p d p`; for Resupply, use `A p d d d p`. Start those runs from fresh card copies. The saved command JSON files record the exact original invocations; their absolute paths must be adjusted when the evidence directory moves.

A newly created card has a new store identity. Compare the original object/revision reference within that card rather than requiring its store UUID or candidate payload hash to equal this capture. Route geometry, progress, costs, and preview extent should match. The inspector source is `inspect_card.rs`; it opens the card read-only and checks all retained candidate routes.
