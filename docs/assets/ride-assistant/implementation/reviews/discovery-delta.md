# Discovery review: delta verification

Status: READY for the reviewed discovery/overview/UI plan. No unresolved D-series blocker.

Checked only the changes for D1–D4 and their immediate query/visit/landmark interfaces. No builds, edits to the repository, tests, or publication.

- **D1 resolved:** RA06 caps source intake at eight nearby plus eight corridor entries and at most 16 distinct visit plans per explicit refresh. It marks bounded ranking Partial, keeps More places independently pageable and plans only the selected review. RA05 explicitly limits a visit to six graph-leg searches, including one possible reconstruction. Existing step/node limits and acknowledged release remain mandatory. Initial distances/counts are clearly tunable policy, not owner-approved design thresholds.
- **D2 resolved:** RA02 binds initial eligibility, query generation, stable identity/order and continuation; current visible/selected rechecks suppress closed entries without reranking. Selected closed state is inert. Newly opened entries wait for explicit refresh. Clock-authority/offset change cancels eligibility generation and preserves identity for restoration. RA06/RA07 share that contract. Loaded-coverage completeness is separately stated.
- **D3 resolved:** RA07 and the epic graph both include RA06; route-window work may still start earlier, with details integration awaiting its owner.
- **D4 resolved:** RA02 reserves summit category7/subtype19, assigns Train category8/subtype20, derives masks from actual service IDs and tests all seven services versus summit isolation.

Immediate interface delta is coherent: RA02 produces one map-bound accessible-coordinate/source/profile-mask record from explicit topology; RA05 validates and consumes it. RA09 depends on RA02 and RA08 and reuses that producer for non-service landmarks. It also reuses/remaps the schedule pool for standalone landmark hours. No new service or runtime source-join subsystem is introduced.

This is plan readiness, not an assertion that implementation/resource/runtime acceptance has passed. RA12 retains its prior full-review readiness; no unrelated second full review was performed.

## Exact reviewed SHA-256

- `epic.md`: `1909b9349c18e85c81afc35f386c7ef1c1ddb87732831ead653b2a94a647dcbe`
- `02-place-queries.md`: `c78a6b36d80b9573150ffdaf4f7d97a89f69a71c681105d333097c30ee856c6a`
- `05-visits.md`: `bb31c0a838a1b6272e2add14a83b2dcaf7810032bac353fec57653c9a50ce9b3`
- `06-find-place.md`: `b25104e8396221b606538e86c7d75b769f19b3a7f8cb6cd30118c05f5b6bf8f0`
- `07-whats-next.md`: `3ef51119bfc21c76667faa745b6a0968020625d645e89b52effacb0877bef3ae`
- `09-landmark-map.md`: `d271e1bd6c00147fe7cd796d18d7ee51483c2bfeee31ad0792c223636bbbd881`
- `12-production-entry.md`: `8ed89a72d8cf0b519d0976ed71e3639108978ac2e3e4496d7bec75562d94d15c`
