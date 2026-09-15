# RA09 CI snapshot evidence

[CI run 34955223736](https://github.com/timohueser/OpenBikeComputer/actions/runs/34955223736) at
`65a53988` produced 259 frames in artifact `ui-snapshots-1` (ID `10391655192`). The only failed
test-job step was the snapshot manifest check: 19 changed frames, no missing or extra frames.
Manifest commit `8ab589e8` records hashes calculated from this run's downloaded PNG files.

The 14 route-related changed hashes each match the reviewed RA04 manifest at `ec581b7f`.
They cover Climb, Detour, the route elevation profile, route/trip totals, route overview and
Statistics ETA, waypoint Statistics, and the legacy Up ahead views. This comparison checked
every changed PNG against that reviewed hash; it did not copy an unverified manifest.
These remain layout captures, not evidence for a useful Detour alternative or the final
ordinary Assistant journey. The integrated real-data acceptance supplies that evidence.

The five additional Firmware frames (`firmware.png`, `firmware-recording.png`, and the German,
Spanish, and French variants) were inspected individually. They show the expected map format
change from v15 to v16. The labels and layout remain legible.

The manifest check passes against all 259 downloaded CI frames. The suite registry and
documentation link check pass. No public conceptual documentation changed. No local snapshot
sweep, Rust test suite, simulator build, shipping image, or base image was run for this correction.
The independent orchestrator review and a green CI run are required before merge.
