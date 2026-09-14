# Review record

Status: **ready for implementation planning handoff**. Three independent reviewer agents checked
the epic and every child. The root agent reconciled interfaces and applied fixes. One substantive
round found blockers; targeted delta checks closed them. This is review of a plan, not proof that
implementation, resource use or physical tests have passed.

## Coverage

| Reviewer | Full child review | Parent and interface review | Evidence |
| --- | --- | --- | --- |
| Discovery | RA02, RA06, RA07, RA12 | Whole epic, every dependency header, data/visit/content integration | [Round](reviews/discovery-review.md), [delta: ready](reviews/discovery-delta.md) |
| Landmarks | RA01, RA08, RA09, RA10, RA13 | Whole epic, every dependency header, hours/access/render interfaces | [Round](reviews/landmark-review.md), [delta: ready](reviews/landmark-delta.md) |
| Routing | RA03, RA04, RA05, RA11, RA13 | Whole epic, every dependency header, query/access/storage interfaces | [Round](reviews/routing-review.md), [delta: ready](reviews/routing-delta.md) |

The reports contain exact reviewed file hashes. Later publication adds GitHub dependency links;
these do not change the implementation requirements. Earlier “not ready” findings in the round
reports are retained with their final resolutions below and in the delta reports.

## Findings and resolutions

| Finding | Resolution |
| --- | --- |
| D1: unbounded total shortlist planning | At most eight nearby plus eight corridor inputs, 16 distinct visit plans; More places plans only the selected review. Each visit has a separate six-search ceiling. |
| D2: opening changes conflict with frozen paging | Stable identity/order and initial eligibility snapshot; current visible/selected status rechecks; closed rows suppressed, selected item inert, explicit refresh for new candidates. |
| D3: missing shared-details dependency | RA07 now depends on RA06. |
| D4: train IDs collide with summit assumptions | Reserve summit IDs; Train category 8/subtype 20; derive masks from actual service IDs. |
| L1: photo rows have no redraw lifetime | Shared mutable target phase writes existing Frame64; bounded retained decoder history; rebuild on fresh target/Sources/base redraw; no second framebuffer. |
| L2: required credits may not render | Separate bounded attribution gate, complete imported notices, supported glyphs and URL representation; omit unsupported assets with reasons. |
| L3: landmark-only hours can lose their pool reference | Shared schedule encoding/interning/remap, including sites without service POI records. |
| R1: stale six-hold limit | Current five holds and two reservations are binding; count phases and refuse incompatible work. |
| R2: checkpoint can erase archive proof | One serialized Metadata mutation path preserves both sections; Navigator and RetentionMachine keep separate policy ownership. |
| R3: uncertain SD commit and cancel races | Definite-unpublished, verified-committed and uncertain-fenced outcomes; cancellation orders behind submitted writes; reconcile phase acknowledgements. |
| R4: naïve visit assembly needs three slots | Final reservation A plus reusable leg B; explicit emitter workspace placement gate and bounded variant reconstruction. |
| R5: restart and original source lifetime undefined | Explicit Assistant Resume only; protect original dependency until completed checkpoint, then release it. |
| R6: Assistant could undo an accepted blockage | Persist unresolved-avoidance provenance and refuse Assistant replanning on that context; no new closure database. |
| C1: incomplete terrain can prove false savings | Complete comparable elevation for ascent claims/caps; seven total easier-route trials; preserve non-ascent profile penalties. |
| C2: waypoint identity changes on second transform | Carry original route-key/ordinal provenance through later transforms. |
| C3: access joins have no runtime producer | Packer resolves bounded coordinate/source/profile-mask approach records; service and landmark paths require real successful visits. RA09 also depends on RA02. |
| C4: excessive visit/recovery scope | Easier route unavailable during a visit; explicit recovery, no general auto-resume or exactly-once durable notification system. |

No review finding remains open. The issue acceptance criteria retain the work that cannot be
proven in a prose review: actual arena placement, codec measurements, source extraction coverage,
route/metadata fault behavior, real-data simulation and physical SD/stack checks.
