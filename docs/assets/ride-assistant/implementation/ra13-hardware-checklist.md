# Ride Assistant physical-device acceptance

Status: **pending**. No device is connected. Simulator results do not pass this
checklist. Continue software integration and offline validation independently.

## Required handoff artifacts

- [ ] Final reviewed commit and branch; green CI links.
- [ ] One final default shipping ELF and linker map, with SHA-256 hashes and exact build command.
- [ ] Resource report against the recorded baseline; unchanged capacity and stack limits.
- [ ] Pinned Swiss, Monaco, and West Cork map packages with card recipes and source hashes.
- [ ] Authored GPS replay files identified as replay, plus their offline simulator evidence.
- [ ] Persistent simulator card copy for each scenario; final normal Assistant entry commands.
- [ ] Named 240 × 320 frames and integrated adversarial review, including resolved deltas.

## Device session

Use the final artifact and normal buttons. Record the result, trace, source IDs,
and elapsed time for each item. Keep any failed item open.

- [ ] Open Assistant with Up + Select. Check all four questions, disabled placeholders,
  Bluetooth in Settings, existing detour, Back, and repeated filter changes.
- [ ] Check the longest supported UI labels, unit changes, grade colors, article line breaks,
  full source notices, and image identity on the actual display.
- [ ] Measure cold and warm SD photo latency. Repeatedly change sites and return from Sources.
  Confirm no stale image pixels appear after source replacement or a read failure.
- [ ] While recording, choose a real rural stop, inspect complete measured cost, accept once,
  arrive, dwell, return/rejoin, and finish. Confirm one continuous recording and no route
  activation at the phase boundary.
- [ ] Restart during outbound, at the stop, and on return. Require explicit Assistant Resume
  with matching sources. Refuse an ambiguous loop or changed map/route/profile.
- [ ] Cancel planning, selected review, and storage publication at each supported stage.
  Confirm source holds and reservations are released and the arena can serve the next task.
- [ ] Exercise full storage, SD removal/read failure, and interrupted checkpoint publication.
  Confirm visible failure or uncertainty fencing, preserved archive proofs, and recovery.
- [ ] Measure stack high-water through the deepest combined ride/photo/plan/store paths.
  Confirm the unchanged floor and uninterrupted sensor updates, buttons, and recording.

Use the board README for flash and trace commands. Fill the final artifact fields
after the integrated shipping build. Do not run a separate base image build or
claim a new hardware high-water measurement from a simulator or compile-time census.
