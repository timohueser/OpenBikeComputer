# Find and preview refinement

The device feedback requests a continuous progress indicator, reuse of measured routes, clear
preview markers, and clearer labels. Find keeps route-cost ranking for every candidate. No
simulator-only planner acceleration is included.

## Behavior

- The normal menu label is **Find a place** in all four languages.
- **Finding places** remains visible through query, planning, publication, and release. The next
  queued candidate starts without waiting for the next GPS fix. The planner's work budget is unchanged.
- Find stores measured routes on the card. Opening a result and going Back to reselect load the
  exact route revision and preview shape without another planner search. Changed inputs refuse reuse.
- A bounded list owns the stored candidates. Category exit and pruning remove unused routes.
  Restart reconciles unaccepted Assistant routes through the existing catalog cleanup mechanism.
  Ordinary routes, the active route, and both accepted checkpoint sources remain protected.
- The preview route is thinner. The destination pin and rider are drawn above it. For mapped
  approaches more than 100 m from a place coordinate, a dotted connector and **Route end to pin**
  caption show the direct gap. This is not a measured walking path or an entrance claim.
- Landmark page counts appear in the top-right corner. The place detail button says **Preview route**.

![Landmark detail](landmark.png)
![Mapped approach and feature pin](gap-preview.png)

These two frames use the real offline West Cork map and the production simulator paths.

## Checks

Passed:

- `CARGO_TARGET_DIR=... tools/obc test -p obc-app -p obc-host-core`
- `CARGO_TARGET_DIR=... cargo clippy -p obc-app -p obc-host-core -p obc-sim --all-targets -- -D warnings`
- `CARGO_TARGET_DIR=... tools/obc test fixtures -p obc-sim`
- `tools/obc test affected --base origin/develop --dry-run`
- One `firmware/ui-snapshots.sh` sweep: 263 frames; 12 intentional changes, no missing frames.
- `tools/obc suites check`, workspace and standalone formatting, and documentation link checks.
- One successful head device measurement against the recorded baseline. No base image rebuild.

The lifecycle test executes nine scenarios, including Back/reselect without a new route plan,
missing retained source, cancellation, acceptance during exit, interrupted ranking, 24 orphan
candidates after restart, preservation of an ordinary route, and accepted checkpoint recovery.
The added recovery delta passed the whole App and host suites and their Clippy checks.

Independent review found stale-store cleanup and restart orphan problems. Both are fixed and their
deltas were reviewed. Named-frame review and the board fallback delta passed with no open findings.

The full local CI suite and wake-profile isolation sweep were omitted. Focused suites cover the
changed packages; the device run checks the changed scheduling path. CI remains the merge gate.

## Device image

The `debug-uart` image was built from `3fc026699` with the normal standalone board configuration.
Its SHA-256 is `c21759c4e2129ec31aa1621b9a67d8488295e61674807823d6584c4f291ba8c9`.
The report-only resource image is separate and must not be flashed.

[Resource guards](resources.log) pass: 309,832 B linked resident, 132,096 B `.uninit`, 9,776 B largest
guarded poll frame, and 49,592 B residual main stack. Resident use is 512 B above the prior
`6c81ddb` debug image. The recorded RAM and stack limits are unchanged.

The card keeps the Swiss map, Meiringen route, and fixed location from the
[previous device setup](../next-place-device/README.md). The SD card is not formatted again.

Device timing and owner acceptance are recorded below after the reload.
