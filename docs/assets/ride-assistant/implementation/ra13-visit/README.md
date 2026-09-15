# Swiss Visit and one recording

The production simulator passed the full Camping Aareschlucht Visit: choose the real place,
review and accept the route, ride to the stop, wait, return, rejoin, and Finish/Save. The strongest
run starts with a newly created card and completes all these actions in one process. It uses normal
buttons, the shared planner, actual card writes, and `GpxPlayer` sensor input. Network access was
denied with `sandbox-exec` for each simulator invocation.

The motion is authored from the accepted route, at 5 m/s with a 60-second stop. It is not a field
recording. The CSV helper reads the actual accepted OBCR and its Visit anchors through the shared
reader; the GPX helper assigns only positions, elevations and timestamps. Neither helper writes
navigation state, arrival events, route costs, or card contents.

## Result

The accepted route is 11,410 m. Its stop is at 5,354 m and its forward rejoin is at 10,091 m.
The original route is 2,319 m, so the complete additional distance is 9,091 m. The preview displayed
its measured arrival climb and retained unknown additional climb; this run does not claim complete
terrain coverage. The map is the published v16 Swiss regional crop, SHA-256
`ebe53f369a558e2c4e8da593b05433e55f730d83d07a928d46f7a236ec145bba`.

| Check | Durable evidence |
| --- | --- |
| Accepted | [Outbound, exact route/source/anchors](accepted-card.log) |
| Stop, at 1,100 s | [AtStop](at-stop-card.log), [arrival card](arrival.png) |
| Return, at 1,400 s | [Returning](returning-card.log); the ignored arrival card remains informational |
| Rejoin, at 2,100 s | [Following, original cleared, lower bound 10,091 m](rejoined-card.log) |
| Finish, at 2,342 s | [One finished ride, no live recorder or journey checkpoint](finished-card.log) |

The phase checks used separate clones of the accepted card. The final fresh-card run completed
without an intervening restart. Its saved ride has 401 samples, exactly one segment, monotonic
0–2,342,000 ms timestamps, and the final coordinate 8.196681 E / 46.723856 N. Object 3, revision 1
remains the sole ride. Its payload is 8,104 bytes with CRC-32 `36b62f37`. The UTC start is
1,750,068,000 (2025-06-16 10:00 UTC), with simulator offset +120 minutes. The saved summary is
11,253 m, 2,283 moving seconds and 320 m climb. The replay uses the existing bounded frame step
(5.855 s for this trace); recorded position chords differ from the route's full geometry. These
recorded totals do not replace planner costs.

A separate restart run continued the same existing zero-second recorder and passed all phases and
Save. Its original empty journal had no durable clock anchor, so the saved start time correctly
remained unknown. Both runs saved the final partial sample batch through normal Finish input.

The final simulator executable was built from `c9e64670` (documentation at `e813d798`), including
reviewed recovery fixes through `92118969`. SHA-256:
`7894149486f4d20727bea20ce73c659ab67f2b958b146cd8c0722540fff5f11c`.
The authored GPX generated below is `ebee7085433ffad187680e2d46609a07f8533478f7007cd3762eadc25e5aa3e2`.
The final exported ride GPX is `d5586fdf5b547d5ec065572ebc470c6dc0e80882dcb087a8fabf7c1d85d09332`.
Card store IDs are random; accepted-route CRCs include those source identities and need not match
between fresh cards. The geometry and anchors do match.

## Reproduce from cached public inputs

Run from the repository root on macOS. Use a new output directory. Fetch the immutable regional
package before denying network access. An isolated fixture cache avoids changing another branch's
`by-id` links. The simulator never fetches source data.

```sh
export OBC_FIXTURE_CACHE="$PWD/.artifacts/visit-cache"
./tools/obc fixtures sync sim-assistant-meiringen
./tools/obc fixtures verify sim-assistant-meiringen
cargo build -p obc-sim --locked
SIM="${CARGO_TARGET_DIR:-target}/debug/obc-sim"
FIXTURES="$(python3 tools/fixtures.py root)"
MAP="$FIXTURES/sim-assistant-meiringen/meiringen.obcm"
WORK="$PWD/.artifacts/visit-proof"
mkdir -p "$WORK/routes" "$WORK/exports"
GPX="$PWD/fixtures/sources/ride-assistant/replays/meiringen-forward-rejoin.gpx"
SCRIPT='p p p p T Q d p p d p f f f f f f f f f f f f p f p f p f'
run_offline() { sandbox-exec -p '(version 1) (allow default) (deny network*)' "$@"; }
run_offline "$SIM" --import "$GPX" --routes-dir "$WORK/routes"
run_offline "$SIM" "$MAP" --routes-dir "$WORK/routes" --create-card "$WORK/accepted.obc" \
  --clock 2025-06-16T10:00 --utc-offset-min 120
run_offline "$SIM" --card "$WORK/accepted.obc" --boot --tracks-dir "$WORK/exports" \
  --gpx "$GPX" --script-at 0 --at 0 --clock 2025-06-16T10:00 --utc-offset-min 120 \
  --script "$SCRIPT" --expect-screen Map --png "$WORK/accepted.png"
```

The script opens the imported route and starts recording. `T` polls its actual GPS position, then
the normal Assistant Find card selects Campsite. The `f` inputs allow its bounded query/planning
work to finish. The final inputs open Camping Aareschlucht, show its Visit preview, and accept it.
An explicit camera position alone cannot replace the GPX poll after route activation.

Build the read-only inspector against the same checkout's just-built dependencies. It opens the
card read-only, rejects every block write, checks metadata and sample CRCs, and can create a new
CSV file for the accepted route. It does not modify the card.

```sh
python3 - <<'PY'
import os
from pathlib import Path
import subprocess
root = Path(os.environ.get('CARGO_TARGET_DIR', 'target')) / 'debug/deps'
args = ['rustc', '--edition=2021', '-D', 'warnings',
        'docs/assets/ride-assistant/implementation/ra13-visit/read_card.rs',
        '-L', f'dependency={root}', '-o', '.artifacts/read_card']
for name in ['obc_storage', 'obc_formats', 'obc_app', 'obc_crc', 'obc_route']:
    artifact = max(root.glob(f'lib{name}-*.rlib'), key=lambda p: p.stat().st_mtime)
    args += ['--extern', f'{name}={artifact}']
subprocess.run(args, check=True)
PY
.artifacts/read_card "$WORK/accepted.obc" "$WORK/accepted.csv" > "$WORK/accepted-card.log"
python3 docs/assets/ride-assistant/implementation/ra13-visit/author_motion.py \
  "$WORK/accepted.csv" "$WORK/accepted.gpx"
```

Now create the final fresh card. The same normal selection/acceptance script runs before motion;
the generated GPS trace then follows the real accepted geometry. The final script pauses, selects
Finish, holds to confirm, and drains the actual save operation. There is no recorder restart.

```sh
run_offline "$SIM" "$MAP" --routes-dir "$WORK/routes" --create-card "$WORK/final.obc" \
  --clock 2025-06-16T10:00 --utc-offset-min 120
run_offline "$SIM" --card "$WORK/final.obc" --boot --tracks-dir "$WORK/exports" \
  --gpx "$WORK/accepted.gpx" --script-at 0 --at 2342 \
  --clock 2025-06-16T10:00 --utc-offset-min 120 --script "$SCRIPT" \
  --script-after 'p f d h f' --expect-screen Home --png "$WORK/finished.png"
.artifacts/read_card "$WORK/final.obc" > "$WORK/finished-card.log"
```

To inspect phase persistence or recovery, clone `accepted.obc` with `cp -c` into a new path. Run
that clone with `--script 'f p T f p f'`, `--script-at 0`, and an endpoint of 1100, 1400 or 2100.
Keep the same clock/offset and authored GPX, and omit `--script-after`. The first Select continues
the zero-second recorder; the second explicitly resumes the accepted Visit. Inspect the resulting
card with `read_card`. These phase probes stop before Save and may leave an unsaved partial batch;
the full final command above saves that batch in the same process.

## Permanent Journey snapshot

`firmware/ui-snapshots.sh` now creates a fresh card from the already cached West Cork map,
accepts the real 110 m landmark destination through normal controls, then reopens its durable
checkpoint. [The checkpoint proof](cork-checkpoint.log) contains no recorder. The resulting
[Resume frame](resume.png) has SHA-256
`1c9b9abea72f8fcb79f9124519c959ac3023e093020717d8bb6df4893e347aee`.
The recipe uses no private input card and adds no new fixture dependency. Temporary card and
intermediate frame paths are removed by the existing cleanup trap.

## Validation boundary

The post-replay control passed the whole simulator suite with external fixtures (71 native tests
and 11 repaint contracts), all-target/all-feature Clippy, firmware Python (86), the suite registry,
formatting and documentation links. Its focused contract uses normal buttons and the actual
FlatRideRecorder to save one segment with its final fix and clock. Independent review approved
`c9e64670` / `e813d798` without findings. The Journey recipe is a separate commit, `4b9e79a0`.

This work did not run a snapshot sweep or build a shipping board image. The orchestrator owns those
final gates. Physical buttons, SD timing, sensor continuity, power loss and stack high-water
acceptance remain pending on hardware.
