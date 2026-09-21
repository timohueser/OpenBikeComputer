# Ride Assistant physical acceptance

Status: pending. The device is not connected. Simulator and CI results do not mark any item below
as passed. Record the tested firmware commit, ELF hash, map hash, device, card, and result with each
session. Use the build, transfer, and diagnostic procedures in the
[board README](../../../../firmware/obc-fw-nrf54l/README.md).

## Prepared data

Use the immutable packages and normal scenario commands in the
[source README](../../../../fixtures/sources/ride-assistant/README.md). Transfer the same maps and
routes with the existing card tools. Keep a separate test card when testing media failures.

| Region | Map SHA-256 | Main check |
| --- | --- | --- |
| Meiringen | `ebe53f369a558e2c4e8da593b05433e55f730d83d07a928d46f7a236ec145bba` | Find, full visit, return, rejoin, recording, Easier |
| West Cork | `a48ebe53b9a545492b94ef4d59cdd2f371e70705683f092d9370112cccc29b23` | Dunlough Castle text, photo, Sources, explicit access |

The Swiss Easier scenario is a real road-network alternative.
Its GPS motion is authored replay. It is not a recorded field ride. Missing hours, terrain,
or mapped access must remain explicit on the device as they are in the simulator.

## Session checks

- [ ] Check all four physical buttons, both drawers, Back, and held-button behavior. Read Find,
  What's next, Landmarks, Easier, Visit review, and Resume in sunlight. Check long labels and
  metric and imperial totals without changing the approved layout.
- [ ] Open Dunlough Castle, read both text pages, open the photo, and read the full Sources.
  Measure the first and repeated photo read on the real SD card. Confirm that text and Back
  remain usable after a photo read failure. Select another landmark and return to the map.
- [ ] Start one recording. Preview a real stop and confirm that navigation does not change before
  acceptance. Accept, reach the stop, dwell, depart, rejoin, and save that same recording.
  Confirm one arrival notice and no extra route computation at arrival.
- [ ] Restart at outbound, stop, and return positions. Continue or discard recording recovery
  independently of the saved journey. Navigation must stay inactive until explicit Resume.
  Refuse ambiguous route positions and changed source bytes without erasing a valid checkpoint.
- [ ] Cancel a visit at its departure and after departure. Check the original route or the
  reviewed connector, as applicable. Repeat selection and cancellation while planner and photo
  work share the existing arena. Change the map, route, or bike profile during pending work.
- [ ] Exercise a full card, a failed checkpoint write, and a disconnected card through the
  existing test procedures. Check retry and source validation. A failed write must not silently
  replace accepted navigation or stop the recording.
- [ ] Measure stack high-water during the full journey, photo reads, route changes, and save.
  Check uninterrupted GPS, sensor, and recording work. Compare the measured result with the
  unchanged resource gates; retain the trace and exact image identity.

Keep failed or untested items open in [RA13](https://github.com/timohueser/OpenBikeComputer/issues/1748)
and the [epic](https://github.com/timohueser/OpenBikeComputer/issues/1734). No simulator result
substitutes for physical readability, SD latency, hardware failures, or measured stack use.
