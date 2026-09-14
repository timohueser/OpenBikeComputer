# Nearby landmarks: simulator study

These are native 240 × 320 captures. Landmark text comes from the sources below. Positions and
access routes are synthetic, placed inside the loaded map. Do not use these captures as access
advice for the real places. The study does not add a landmark backend or map format.

See the [simulator README](../../../../apps/obc-sim/README.md#landmarks-study) for controls.
Next town is removed from the Assistant questions.

| Nearby map | What it is | Interesting fact |
| --- | --- | --- |
| ![Map with three landmarks and the selected card](nearby.png) | ![Aare Gorge description](aare.png) | ![Aare Gorge formation and walkways](aare-facts.png) |

| Waterfall | Literary connection | Source |
| --- | --- | --- |
| ![Reichenbach Falls description](falls.png) | ![Fictional Holmes and Moriarty connection](falls-facts.png) | ![Text attribution opened from the bottom drawer](source.png) |

| Railway | History | Visit preview | Arrival |
| --- | --- | --- | --- |
| ![Gelmerbahn description](gelmer.png) | ![Railway construction and public access dates](gelmer-facts.png) | ![Shared visit cost including return](preview.png) | ![Return guidance already active on arrival](arrival.png) |

Up/Down selects a marker, then changes pages after Select opens the text. Back preserves the
selected landmark. **Down + Back → Sources** opens attribution, the licence URL, and article
URLs without adding pages to each description. Back restores the reading page. **Visit** opens the shared route preview. **Add stop** accepts the complete
visit. Reading another place leaves that accepted destination and return route intact.

Known-closed landmarks are excluded. These three examples have unknown hours. Distances on the
nearby card are straight-line distances from the rider. Route costs in the preview come from the
prepared mock route legs. Three results are a fixture limit, not a production policy.

## Text sources and licence

The landmark descriptions in the simulator and these images are adaptations of text by
Wikipedia contributors. The descriptions are shared under
[Creative Commons Attribution-ShareAlike 4.0](https://creativecommons.org/licenses/by-sa/4.0/).
They are shortened and reworded for the device. The application code retains its repository licence.

| Device name | Article revision | Selected material |
| --- | --- | --- |
| Aare Gorge | [Aare Gorge, revision 1327206534](https://en.wikipedia.org/w/index.php?title=Aare_Gorge&oldid=1327206534) | Limestone gorge, wall height, glacial meltwater, walkways since 1889. |
| Reichenbach Falls | [Reichenbach Falls, revision 1362225684](https://en.wikipedia.org/w/index.php?title=Reichenbach_Falls&oldid=1362225684) | Upper fall height and the fictional events in Conan Doyle's 1893 story. |
| Gelmerbahn | [Gelmer Funicular, revision 1322069534](https://en.wikipedia.org/w/index.php?title=Gelmer_Funicular&oldid=1322069534) | Railway to Gelmersee, 106 percent maximum gradient, 1926 construction, public opening in 2001. |

The device prototype has no photographs. The separate [photo study](../landmark-photos/README.md)
compares device-sized images and estimates their storage cost. A later implementation could
preconvert a small image during map creation and store palette pixels. For example, 160 × 90 at one byte per pixel is 14,400 bytes before
metadata or compression. This is a possible design, not an implemented image format.

## Verification

```sh
cargo check -p obc-sim
./tools/obc test -p obc-app -p obc-sim
cargo clippy -p obc-app -p obc-sim --all-targets -- -D warnings
cargo build -p obc-sim
./tools/obc suites check
./tools/obc test affected --base origin/develop --dry-run
python3 docs/build_docs.py --check-links
cargo fmt --all
cargo fmt --manifest-path firmware/obc-fw-nrf54l/Cargo.toml
cargo fmt --manifest-path firmware/obc-boot/Cargo.toml
cargo fmt --manifest-path apps/obc-desktop/Cargo.toml
git diff --check
```

The focused app and simulator suites passed. Tests cover nearby ordering, opening-state
filtering, selection retention, and the accepted landmark visit through arrival and rejoin
while browsing another place. Recording remains active in the same session.

Ten named frames were captured and inspected, including all six article pages. The nearby map
was captured again after its layout changed. These are targeted frames, not a full UI sweep.
The headless scripts also check the expected screen. No backend or board behavior is claimed.

The affected dry run includes unrelated changes from this handoff branch's older base.
The full affected plan was deliberately omitted. No full workspace gate, external-fixture suite,
resource measurement, board build, browser test, or hardware test was run.

## Source drawer refinement

| Bottom drawer | Licence URL | Article URL |
| --- | --- | --- |
| ![Sources action in the bottom drawer](sources-drawer.png) | ![Licence URL](licence.png) | ![Article URL](article-source.png) |

The descriptions now have two pages. Attribution remains accessible from the nearby map and
reading pages. The source view lists all three study articles. This uses the existing drawer,
and does not create another button combination. The source action and Back leave navigation
and recording unchanged. The relevant guidance is
[CC BY-SA 4.0 section 3(a)(2)](https://creativecommons.org/licenses/by-sa/4.0/legalcode.en#s3a)
and [Wikimedia's text reuse terms](https://foundation.wikimedia.org/wiki/Policy:Terms_of_Use#7._Licensing_of_Content).
They permit attribution appropriate to the medium and attribution through article URLs.
The drawer is our interpretation of an accessible placement for this small offline screen.

The focused app and simulator tests, Clippy, package build, format commands, registry check,
and documentation link check listed above passed for this refinement. Eleven named captures
check the updated reading pages, drawer, source URLs, and Back restoring the second text page.
The same full-workspace and hardware checks remain deliberately omitted.
