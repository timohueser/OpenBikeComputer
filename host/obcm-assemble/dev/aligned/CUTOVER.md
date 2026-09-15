# Cutover inventory — not executed

The candidate was not adopted. These are the concrete files and operations that
a hard format cutover would need. None is an outstanding step of the retained
experiment, and none was performed. This inventory prevents the prototype's
v16 input adapter from being mistaken for a complete shipping transition.

| Surface | Required hard-cutover work |
| --- | --- |
| Runtime | Assign the shipping OBCM version; remove all v16 input acceptance and version-250 experiment code. Keep packer, assembler, reader, planner, and full validator on one format. |
| Exact format | Update `specs/OBCM_Spec.md` header and navigation contracts for aligned physical references, dense identity, padding, bounds, and sentinels. Review OBCA and protocol version contracts. |
| Web assembly fixtures | Rebuild all five cells and both expected maps under `apps/obc-web-assemble/tests/fixture/` through the real producer. |
| Embedded maps | Rebuild `apps/obc-sim/assets/grimsel-demo.obcm` and `host/obc-bake/assets/teningen-preview.obcm`; these also feed web demo, host-core, and preview paths. |
| External fixture packages | Rebuild `sim-grimsel`, `sim-monaco`, `sim-assistant-meiringen`, and `sim-assistant-west-cork`; update archive digests, `fixtures/catalog.toml`, build provenance, and `fixtures/build-assistant-package.py`'s format guard. |
| Protocol vectors | Regenerate version-read vectors and `specs/vectors/manifest.json`, including variants with and without map storage. |
| Public cells | Rebuild the coherent catalog with the new producer, including both skins and all bands. Existing baker cache keys include the format version; stale cells must not enter the new catalog. |
| Published contracts | Run the baker's strict catalog verification, publish referenced objects first and the root last, and verify the public catalog and object hashes before shipping its reader. |
| Device acceptance | Run the recorded static resource gates and report physical-device timing/energy as unmeasured unless actual hardware is tested. |

The inspected public catalog is OBCM v16, generated on 2026-09-15 at 10:45:59Z.
It has one region, `europe/germany/baden-wuerttemberg`. The pinned workload here
uses one coarse selection containing 37 map cells and four terrain cells; it is
not the complete published catalog. The earlier documented publication in
`docs/assets/ride-assistant/implementation/catalog-v15-publication.md` covered
215 cells across four bands, two skins, and 28 terrain cells. That historical
object count is not a claim about the current v16 root.

The publisher in `host/obc-bake/src/publish.rs` defines the content-first,
root-last transaction. It accepts a local directory target for reviewable
artifacts and an R2 target for publication. The bake workflow documents the
workstation requirements for large region builds. Fixture publication has its
own separate publisher and credentials.

Publication access would be a prerequisite for an adopted cutover. It is not
needed to merge this retained result. No external catalog or fixture object
was changed. The retain decision is based on the measured runtime and producer
tradeoffs; publication work is not a reason to reject the design.
