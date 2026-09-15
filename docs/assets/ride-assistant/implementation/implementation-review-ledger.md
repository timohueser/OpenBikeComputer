# Ride Assistant implementation and review ledger

Status snapshot: 2026-09-15, after controls #1769 and scenarios #1767 merged. This ledger combines
RA13 `115f3804` with catalog evidence `dc34410a`. It records implementation and
independent adversarial reviews; it is not a new review or a CI approval.

The [reviewed plan](README.md) at `0b5f50eb` is the scope reference. Its plan-review
files establish design readiness only. The links below identify implementation
reviews and the fixes that closed their findings. Commits identify substantive
changes; they are not claims that each listed commit was the final CI head.

## Thirteen children

| Child / issue | Implementation PRs and code | Independent review evidence | Retained software evidence | Remaining physical work |
| --- | --- | --- | --- | --- |
| RA01 [#1736](https://github.com/timohueser/OpenBikeComputer/issues/1736) — Real inputs | [#1750](https://github.com/timohueser/OpenBikeComputer/pull/1750); `90373c16`, `f51b5533`. | [full review](https://github.com/timohueser/OpenBikeComputer/pull/1750#pullrequestreview-5203328772); [manual-suite fix accepted](https://github.com/timohueser/OpenBikeComputer/pull/1750#pullrequestreview-5203387877); [dependency delta](https://github.com/timohueser/OpenBikeComputer/pull/1750#issuecomment-5671542006). | [Source provenance](../../../../fixtures/sources/ride-assistant/README.md): pinned inputs and cached scenarios. | Transfer the pinned maps to the device; no field capture is claimed. |
| RA02 [#1737](https://github.com/timohueser/OpenBikeComputer/issues/1737) — Place queries | [#1754](https://github.com/timohueser/OpenBikeComputer/pull/1754); `65b47b4b`, `f67dff00`, `eb210515`, `01918e5e`. | [full review](https://github.com/timohueser/OpenBikeComputer/pull/1754#pullrequestreview-5205662128); [four fixes accepted](https://github.com/timohueser/OpenBikeComputer/pull/1754#pullrequestreview-5205750929); [encounter correction](https://github.com/timohueser/OpenBikeComputer/pull/1754#issuecomment-5676753933). | [Query evidence](ra02-encounter-validation.md): continuous passes, paging, chunk seams, bounded memory; [clock evidence](ra02-clock-evidence.md). | Physical button paging, SD failures, and wake behavior. |
| RA03 [#1738](https://github.com/timohueser/OpenBikeComputer/issues/1738) — Route facts | [#1753](https://github.com/timohueser/OpenBikeComputer/pull/1753); `c8050c6d`, `1693c2a1`, `2c5e7638`. | [full review](https://github.com/timohueser/OpenBikeComputer/pull/1753#pullrequestreview-5205687272); [four fixes accepted](https://github.com/timohueser/OpenBikeComputer/pull/1753#pullrequestreview-5205759703). | [Fact validation](ra03-validation.md); final real measured costs in [Swiss Easier](ra13-easier/README.md). | Compare live elevation/profile display and sensor continuity. |
| RA04 [#1739](https://github.com/timohueser/OpenBikeComputer/issues/1739) — Review lifecycle | [#1759](https://github.com/timohueser/OpenBikeComputer/pull/1759); `c76e9aeb`, `f60d9de0`, `7af59b7a`, `1de46eae`, `cb06fb05`. | [full review](https://github.com/timohueser/OpenBikeComputer/pull/1759#issuecomment-5675689289); [remount fix accepted](https://github.com/timohueser/OpenBikeComputer/pull/1759#issuecomment-5675949389); [catalog consumer fix](https://github.com/timohueser/OpenBikeComputer/pull/1759#issuecomment-5676755441). | [Lifecycle evidence](ra04-evidence.md): source holds, acknowledged release, verified acceptance, uncertain writes, current explicit cleanup. | Power loss, card removal/full card, contention and physical stack high-water. |
| RA05 [#1740](https://github.com/timohueser/OpenBikeComputer/issues/1740) — Visits | [#1761](https://github.com/timohueser/OpenBikeComputer/pull/1761); `48653a1e`, `74bc1df0`. Controls [#1769](https://github.com/timohueser/OpenBikeComputer/pull/1769); `b388a284`, `fe2151b0`, `fa50312c`, `a8cf6684`. | [full review](https://github.com/timohueser/OpenBikeComputer/pull/1761#issuecomment-5676339854); [three fixes accepted](https://github.com/timohueser/OpenBikeComputer/pull/1761#issuecomment-5676483448); [real return seam](https://github.com/timohueser/OpenBikeComputer/pull/1761#issuecomment-5677703631). [Controls and recovery review accepted](https://github.com/timohueser/OpenBikeComputer/pull/1769#issuecomment-5679149529). | [Visit contracts](ra05-evidence.md), [controls](ra05-production-controls-evidence.md), [complete recorded Swiss journey](ra13-visit/README.md). | Actual arrival, dwell, rejoin, cancellation, reboot/Resume and uninterrupted recording. |
| RA06 [#1741](https://github.com/timohueser/OpenBikeComputer/issues/1741) — Find place | [#1762](https://github.com/timohueser/OpenBikeComputer/pull/1762); `5a1ef09f`, `67810648`, `8547bf96`. Production entry [#1768](https://github.com/timohueser/OpenBikeComputer/pull/1768). | [source replacement fix](https://github.com/timohueser/OpenBikeComputer/pull/1762#issuecomment-5676999524); [retained-hours fix](https://github.com/timohueser/OpenBikeComputer/pull/1762#issuecomment-5677144885); [real Find tour review](https://github.com/timohueser/OpenBikeComputer/pull/1762#issuecomment-5677770685). | [Tour evidence](ra06-tour-evidence.md); final Swiss normal Find, review and acceptance in [Visit replay](ra13-visit/README.md). | Physical list/detail interactions and response latency during recording. |
| RA07 [#1742](https://github.com/timohueser/OpenBikeComputer/issues/1742) — What's next | [#1763](https://github.com/timohueser/OpenBikeComputer/pull/1763); `9d646b9d`, `f7e1f33a`, `1f842095`. Production entry [#1768](https://github.com/timohueser/OpenBikeComputer/pull/1768). | [full review](https://github.com/timohueser/OpenBikeComputer/pull/1763#issuecomment-5676934345); [three fixes accepted](https://github.com/timohueser/OpenBikeComputer/pull/1763#issuecomment-5677049504). | [Window and timeline evidence](ra07-evidence.md): real route facts, complete paging, failure states and units. | Read moving-route windows, grade colors and page boundaries on the display. |
| RA08 [#1743](https://github.com/timohueser/OpenBikeComputer/issues/1743) — Landmark sources | [#1751](https://github.com/timohueser/OpenBikeComputer/pull/1751), [#1752](https://github.com/timohueser/OpenBikeComputer/pull/1752), [#1756](https://github.com/timohueser/OpenBikeComputer/pull/1756); `c7925c65`, `b7854f48`, `f11bb6bc`, `7b5f9d50`. | [compiler fixes accepted](https://github.com/timohueser/OpenBikeComputer/pull/1751#issuecomment-5671693841); [image selection delta](https://github.com/timohueser/OpenBikeComputer/pull/1751#issuecomment-5671790843); [capture/redirect review](https://github.com/timohueser/OpenBikeComputer/pull/1752#issuecomment-5674987223); [country recount review](https://github.com/timohueser/OpenBikeComputer/pull/1756#issuecomment-5675471347). | [Complete Swiss census](ra13-source-measurements.md). Raw country fixture publication was removed by owner correction #1760; only compiled content and regional maps remain public. | No capture/compiler-specific hardware gate; text and photo display are checked under RA09/10. |
| RA09 [#1744](https://github.com/timohueser/OpenBikeComputer/issues/1744) — Landmark map and photos | [#1755](https://github.com/timohueser/OpenBikeComputer/pull/1755), [#1757](https://github.com/timohueser/OpenBikeComputer/pull/1757), [#1758](https://github.com/timohueser/OpenBikeComputer/pull/1758); `80edd44a`, `031a714d`, `7b3771bf`, `4482e655`, `20a5854d`. | [assembly readback fix](https://github.com/timohueser/OpenBikeComputer/pull/1755#issuecomment-5675402700); [cut fixes accepted](https://github.com/timohueser/OpenBikeComputer/pull/1757#issuecomment-5675591970); [covered-frame fix accepted](https://github.com/timohueser/OpenBikeComputer/pull/1758#issuecomment-5675865461). | [Regional map evidence](ra09-regional-evidence.md), [photo evidence](ra09-photo-ci-evidence.md), [public v16 catalog](catalog-v16-publication.md); real Cork photos in RA10/12. | Cold/warm SD photo latency, overlays, source failure/replacement and arena reuse. |
| RA10 [#1745](https://github.com/timohueser/OpenBikeComputer/issues/1745) — Landmark UI | [#1764](https://github.com/timohueser/OpenBikeComputer/pull/1764); `7bb45cd4`, `db1ec4b9`, `47617697`, `bfb3142e`, `447e347f`. Production entry [#1768](https://github.com/timohueser/OpenBikeComputer/pull/1768). | [full review](https://github.com/timohueser/OpenBikeComputer/pull/1764#issuecomment-5677070970); [three fixes accepted](https://github.com/timohueser/OpenBikeComputer/pull/1764#issuecomment-5677266386). | [Real text/photo/Sources evidence](ra10-evidence/README.md); [production entry frames](ra12-evidence/README.md). | Read all source notices and long text; test photo failure with physical Back/Select. |
| RA11 [#1746](https://github.com/timohueser/OpenBikeComputer/issues/1746) — Easier routes | [#1765](https://github.com/timohueser/OpenBikeComputer/pull/1765); `b6396b8a`, `a77ec8a0`, `160c5291`. Real acceptance [#1770](https://github.com/timohueser/OpenBikeComputer/pull/1770); `d613f826`, `a0177f1d`. | [full review](https://github.com/timohueser/OpenBikeComputer/pull/1765#issuecomment-5677314169); [review/recovery fixes](https://github.com/timohueser/OpenBikeComputer/pull/1765#issuecomment-5677414283); [normal entry delta](https://github.com/timohueser/OpenBikeComputer/pull/1765#issuecomment-5677912429); [real evidence review](https://github.com/timohueser/OpenBikeComputer/pull/1770#issuecomment-5678425121). | [Implementation evidence](ra11-evidence.md); [Swiss comparison and exact accepted bytes](ra13-easier/README.md). | Physical comparison readability, planner responsiveness and accepted guidance while recording. |
| RA12 [#1747](https://github.com/timohueser/OpenBikeComputer/issues/1747) — Production entry | [#1768](https://github.com/timohueser/OpenBikeComputer/pull/1768); `035e9696`, `2cffd2d7`, `26b8f70b`. Final composition [#1766](https://github.com/timohueser/OpenBikeComputer/pull/1766). | [full and fix review accepted](https://github.com/timohueser/OpenBikeComputer/pull/1768#issuecomment-5678074838); [census/frame delta](https://github.com/timohueser/OpenBikeComputer/pull/1768#issuecomment-5678182391); [real Detour assertion](https://github.com/timohueser/OpenBikeComputer/pull/1768#issuecomment-5678419755). | [Production frames and behavior](ra12-evidence/README.md), [CI evidence](ra12-ci-evidence.md). Final composition status is separate below. | All normal gestures, four questions, Settings/Bluetooth, Detour and translations on the device. |
| RA13 [#1748](https://github.com/timohueser/OpenBikeComputer/issues/1748) — Release handoff | [#1767](https://github.com/timohueser/OpenBikeComputer/pull/1767); `fbf5f454`, `9f016807`, `6c33f331`, `115f3804`. Replay [#1771](https://github.com/timohueser/OpenBikeComputer/pull/1771); `08de2fe8`, `c9e64670`. Catalog `dc34410a`. | [recipe/provenance review](https://github.com/timohueser/OpenBikeComputer/pull/1767#issuecomment-5677703368); [Swiss package review](https://github.com/timohueser/OpenBikeComputer/pull/1767#issuecomment-5678058882); [replay review](https://github.com/timohueser/OpenBikeComputer/pull/1771#issuecomment-5678547187); [post-replay delta](https://github.com/timohueser/OpenBikeComputer/pull/1771#issuecomment-5678778840). [Integrated code and real-card evidence accepted](https://github.com/timohueser/OpenBikeComputer/pull/1766#issuecomment-5679149209). | [Packaging](ra13-packaging-evidence.md), [compiled Swiss input](ra13-swiss-content-evidence.md), [Visit](ra13-visit/README.md), [Easier](ra13-easier/README.md), [catalog](catalog-v16-publication.md). | Complete the [device checklist](device-test-checklist.md), including real stack high-water; no hardware item has passed. |

## Merge and CI boundary

At this snapshot, GitHub reports these PRs merged: #1750, #1751, #1752, #1753,
#1754, #1755, #1756, #1757, #1759 and #1761. Production-entry #1768 and Easier
acceptance #1770 are merged into the production line. Replay #1771 is merged
into the controls line. A merge into a stacked branch does not establish a
merge of the final feature into `develop`.

Controls #1769 and scenarios #1767 have since merged after their final CI passed.
Integration #1766 is open at pushed head `7ce64484`; final production CI is running.
Its runtime is unchanged from `115f3804`; the develop merge adds four historical
evidence documents. No final integrated green-CI claim is made here. The
[public integration review](https://github.com/timohueser/OpenBikeComputer/pull/1766#issuecomment-5679149209)
records this source and review boundary.

The earlier study #1749 and implementation PRs #1758, #1762, #1763, #1764 and
#1765 remain open at this snapshot. Their code is carried into production.
They are to be closed as superseded only after final integration is green.
Their older failing checks are not waived, changed to passed, or used as final
acceptance. In particular, the old photo branch lacked production screen
recipes; the later production entry owns that coverage.

## Integrated evidence and open acceptance

The [complete Swiss Visit replay](ra13-visit/README.md) uses a fresh card, public
map data, normal buttons and authored GPS motion derived from the actual accepted
route. It records arrival, dwell, return, rejoin and Stop/Save in one process:
401 samples, one segment, 2,342 seconds and a trusted UTC start. The read-only
card inspector checks stored route identities and the finished ride CRC.
The [Swiss Easier replay](ra13-easier/README.md) compares 5,492 m / 44 m ascent
remaining with 1,034 m / 4 m and checks the reconstructed and accepted bytes.
Neither replay is a recorded field ride.

The [country census review](https://github.com/timohueser/OpenBikeComputer/issues/1748#issuecomment-5677135805)
and [compiled-input review](https://github.com/timohueser/OpenBikeComputer/issues/1748#issuecomment-5677938869)
cover the production measurements and compiled input archive. They do not
establish country-wide routing coverage. The [owner storage correction](https://github.com/timohueser/OpenBikeComputer/issues/1743#issuecomment-5675813576)
removed the raw Swiss archive from the fixture service. This ledger does not
restore that archive or treat an obsolete download command as current.

The [integrated review](https://github.com/timohueser/OpenBikeComputer/pull/1766#issuecomment-5679149209)
and [controls review](https://github.com/timohueser/OpenBikeComputer/pull/1769#issuecomment-5679149529)
report no remaining correctness finding. They include near-stop Resume, refused
and uncertain writes, recording recovery, real-card evidence and title-fit deltas.
They do not replace final production CI. The artifact documentation at `b1ff8209`
is independently reviewed but is not part of this ledger commit. Its retained
shipping ELF predates the four shorter titles and has no emitted linker map;
those limits remain explicit in the artifact handoff.

Physical-device acceptance is pending. No device was connected. The
[device checklist](device-test-checklist.md) carries SD timing, physical buttons,
readability, power/card failure, sensor continuity and measured stack high-water.
The integration owner supplies final image identities, resource evidence and
snapshot acceptance separately. This ledger adds no build, test, image or sweep.
