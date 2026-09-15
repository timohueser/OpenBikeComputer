# Ride Assistant integrated handoff

Software integration: `115f3804` on `codex/ra13-offline-scenarios`. Its runtime tree is identical
to the reviewed acceptance head `fb48e40c`. The final title code is `a8cf6684` (also
`ae4b2741` in the production controls PR). Physical acceptance remains pending.

## Run the production simulator

Use the [source commands and immutable packages](../../../../fixtures/sources/ride-assistant/README.md)
and [simulator setup](../../../../apps/obc-sim/README.md). Cache packages and normal build
dependencies before disconnecting the network. These commands use ordinary map readers,
Navigator, storage, rendering and button input:

```sh
tools/obc sim assistant-forward-rejoin
tools/obc sim assistant-monaco-dense
tools/obc sim assistant-dunlough-access -- --create-card .artifacts/west-cork.obc
```

The persistent-card path must not exist on the first run. Reopen it with the simulator's
normal `--card` option. Replays provide authored GPS motion; they do not inject acceptance,
arrival or costs. The four questions are available through the normal Assistant menu.

## Acceptance evidence

| Case | Result and evidence |
| --- | --- |
| Swiss complete Visit | [Fresh-card, one-process acceptance](ra13-visit/README.md): Camping Aareschlucht, actual 11,410 m accepted journey, arrival/dwell/return/rejoin, normal Stop/Save. One ride, 401 samples, one segment, 2,342 s, trusted UTC start, payload CRC `36b62f37`. |
| Swiss Easier | [Actual network alternative](ra13-easier/README.md): current 5,492 m / 44 m ascent to 1,034 m / 4 m, exact accepted bytes. Elevations are from the captured terrain. |
| West Cork | [Normal production entry](ra12-evidence/README.md): Dunlough Castle Q5315471, source text, actual photo, all Sources, explicit mapped 110 m Visit. Arbitrary origins without mapped access remain unavailable. |
| Dense Monaco and route brief | [Production captures](ra12-evidence/README.md): full categories, More places, source editor, authored waypoints, actual Detour +533 m; [query evidence](ra02-encounter-validation.md) and [timeline evidence](ra07-evidence.md) cover bounded ordering and interval semantics. |
| Restart, cancellation and storage | [Controls and recovery evidence](ra05-production-controls-evidence.md): explicit Resume, phase persistence, uncertain-write fencing, recording recovery independent of saved navigation. Artificial fault tests are identified as tests. |
| Swiss content | [Country census](ra13-source-measurements.md), [compiled content](ra13-swiss-content-evidence.md), and [regional package](ra13-packaging-evidence.md). Country source coverage is separate from regional map coverage. |

Independent adversarial reviews covered each implementation slice and the integrated result.
Findings were fixed and reviewed as deltas. In particular, the final lifecycle changes preserve
accepted progress at the stop, retry definite unpublished writes, and preserve Assistant recovery
when the rider continues or discards a recovered recording. Preview and acceptance keep one
Recorder owner. The final arrival and Resume titles fit all four supported languages.

The [published v16 catalog](catalog-v16-publication.md) was verified against its retained root
and downloaded through the normal coarse-region fetch path into an empty directory.

## Prepared binaries and resource measurement

The local artifact directory is
`/Users/timo/Documents/OSM-agents/ra13-integrated-acceptance/.artifacts/ride-assistant-release/`.
The [artifact manifest](final-artifacts.json) identifies the exact bytes. Binaries are local test
artifacts, not a published release. Map package hashes and clean-cache instructions are in the
source README; the card inspector and complete replay recipe are tracked with the Visit evidence.

The simulator `obc-sim-final` is from `a8cf6684`. Its SHA-256 is
`e165c232d081ce3120aadfe79602a5103a6e815fa18c6e994b666ac46b550706`.
The runtime trees at that source and the integration head match.

The single local shipping build was made at `a53ad86e`, with
`cargo build --release --locked -vv` in `firmware/obc-fw-nrf54l`.
ELF SHA-256: `3803b060210ee3e4f83c37ec88ab446bc49c8559cc149ded4000bb3c594ebc78`.
It contains the complete Visit and recovery implementation. Four later translated titles are
shorter in the final source; the ELF retains their earlier wording. That byte difference is
explicit: the ELF is not claimed to be a build of the final title revision. A second local image
build requires the owner's verification-budget exception. No linker map was emitted by this build.

| Measurement | Recorded baseline | This shipping ELF |
| --- | ---: | ---: |
| Linked resident | 305,192 B | 308,072 B (+2,880) |
| `.uninit` | cap 132,096 B | 132,096 B |
| Shared arena | 131,072 B | 131,072 B |
| Flash | 1,449,768 B | 1,689,024 B (+239,256) |
| Largest guarded poll frame | 9,792 B | 9,784 B |
| Residual main stack | 54,232 B | 51,352 B |
| Largest task body | 1,100 B | 4,040 B |
| Boot-chain ceiling | limit 24,576 B | 16,696 B |
| `init_idle` frame | limit 4,096 B | 176 B |

These are comparisons with the recorded baseline, not a rebuilt base or attribution of every byte
to this epic. The resource and strict-alignment guards passed. The residual stack exceeds the
historical 37,016 B device high-water by 14,336 B; the required 8,704 B margin is unchanged.
That historical high-water is not a measurement of this image. CI independently checks the exact
allocation record: App 52,312 B, navigation arena arm 97,440 B and route index 12,400 B.
Source holds remain at most five and reservations at most two. No country-sized runtime collection
or permanent decoded-photo buffer was added.

The [country measurement](ra13-source-measurements.md) records 1,495 compiled records,
1,119 photos, 9,375,549 compressed photo bytes, 12,550 B p95 and 17,347 B maximum.
The photo reader retains bounded decoder history and uses the existing frame target.

## Remaining acceptance

Use the [physical checklist](device-test-checklist.md). Buttons and sunlight readability,
real SD latency and failures, sensors, and stack high-water require the device. All are unchecked.
The regional Swiss terrain covers 71.6% of its selected whole native cells; missing ascent and
surface facts remain unknown. Many real service objects lack explicit mapped approaches, so
some Visit requests correctly remain unavailable.

The final affected-selection and snapshot log summary will be added when the ongoing run ends.
No physical session, base rebuild, wake isolation, mutant test, or second local shipping image
was run. The final CI aggregate remains the merge gate.
