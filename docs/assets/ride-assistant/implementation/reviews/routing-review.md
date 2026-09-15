# Routing adversarial review — first full round

Status: **Not ready until R1–R6 below are resolved.** Reviewed epic integration and all 13 child headings/dependencies; fully read RA03, RA04, RA05, RA11, RA13. Also checked RA02 access data ownership, current Metadata/storage/route source code, and `specs/Retention_Metadata.md`. No edits, builds, tests, or GitHub changes.

The owner choices are sound: Navigator, ordinary derived OBCR, shared route metrics, explicit acceptance, fixed candidate policies, and retained prototypes. Dependencies form a DAG. Keeping Shorter distance and no easier alternatives during an active visit is a good way to avoid unnecessary scope. The remaining concerns are executable contracts, not a request for another architecture rewrite.

## R1 — Current source-hold limit is FIVE, not six

**Blocker. RA04 Constrained-resource criteria; RA13 Resource budget; any epic wording.**

`firmware/obc-storage/src/flat/store.rs` currently declares `MAX_OPEN_OBJECTS: usize = 5`, `MAX_RESERVATIONS = 2`. `open_objects::ACCOUNTED` also sums to five. Surrounding historical prose and #1700 discuss six; those are stale. My earlier audit repeated that historical count and must not govern implementation.

**Fix:** require the existing five holds and two reservations. Census must count map, active derived route, retained original source, transfer, temporary metadata readback/source, candidate source and route swap. Refuse incompatible concurrent work; do not raise the cap to six. A retained original can consume the available fifth hold during a visit, so bound when a new candidate/transfer can be admitted. Use actual current resource constants/baseline, not issue-era figures.

## R2 — New Navigator checkpoint must not overwrite retention/archive authority

**Blocker. RA04 Durable acceptance and restart; epic shared rules.**

There is no existing navigation checkpoint in Metadata. Current `OBRM` is retention-only: one 7,712-byte image, 64 route/128 ride proof rows, RetentionMachine owns policy, exact rows bind ObjectId+Revision+payload length+CRC. `Metadata::replace` checks its target is a matching row; generic use for a standalone Navigator target does not work today. Every producer rewrites the entire payload, including ARCHIVE_RIDE and timestamp reconciliation. Two domain-owned stale drafts could erase new navigation state or archive proof even if their own content is valid.

**Fix:** state that this issue adds one optional versioned checkpoint section to the existing singleton, with one shared serialized read/modify/publish path. Navigator owns only checkpoint semantics; RetentionMachine keeps exclusive retention ownership. Every metadata mutation preserves the other section and uses the current complete image/CAS. Define full accepted-route fingerprint, not only ID/revision, and checkpoint target validation separately from retention row validation. Keep archive-proof semantics, source matching, complete catalog scan, CRC, readback, and fence intact. Account for the larger maximum encoded image and its existing workspace placement; do not silently grow a permanent App buffer. Add concurrent/archive-versus-checkpoint tests that prove neither section is lost.

## R3 — Acceptance, phase writes, and uncertain publication need concrete outcomes

**Blocker. RA04 Durable acceptance and restart; RA05 Progress.**

“Define stale/cancel handling” and “a refused write leaves a recoverable current phase” defer the most important decisions. Existing Metadata publication can return RemountRequired **after the new generation might be durable**, including failed committed readback. In that case it is incorrect to claim the previous journey/phase definitely remains. Rewriting a phase before presenting it also creates a pending period in which fixes can pass stop/rejoin and a cancel can arrive. Exactly-once arrival-card presentation across crashes is not achievable by ordering a durable write and screen draw alone.

**Fix:** specify three writer outcomes: definitely unpublished (old checkpoint, retry), verified committed (new checkpoint authoritative), and uncertain/fenced (no successful acceptance or compensating delete; retained bytes remain readable, fresh mutations await remount/barrier and recovered-head inspection). Before physical checkpoint submission, an admitted cancel annihilates it; after submission, serialize cancellation behind its resolved durable outcome instead of promising it can revoke an in-flight commit. Do not return “cancelled/unaccepted” while accepted status is unknown. If committed, subsequent cancel is a new accepted-journey change. Apply the same rules to manual route change/clear.

For phases, keep the prepared route readable and recording independent while checkpoint is pending; retain the latest trustworthy progress to reconcile after acknowledgment. A refusal or fence must not produce a silent reroute, repeated arrival, or jump to a later crossing. Define replay-safe phase transitions. Do not promise an arrival card exactly once across reboot; it is informational and can be suppressed on recovery. Require tests at each publication/ACK boundary and for stop+rejoin fixes arriving during a pending phase write.

## R4 — Two-slot construction needs an actual phase algorithm

**Blocker. RA04 resource census; RA05 Route construction.**

A naïve implementation plans outbound into sealed A, return into sealed B, then requires C to emit the derived route. Existing Splicer consumes original+one leg; it does not assemble two independently planned legs with a single unused output slot. “Construct one OBCR” and “serialize within existing limit” do not yet establish feasibility.

**Fix:** select a concrete bounded strategy. A suitable option is one unsealed final output reservation plus one reusable temporary leg reservation: plan a leg, seal it, stream it into the final route emitter, release it, then reuse for the next leg; finally append original tail and emit the waypoint section/header. State where the final emitter state lives while NavPlanner uses its own emitter/scratch, and prove its arena placement in the implementation layout assertions. If another strategy is smaller, name its exact phase ownership instead. Do not introduce temporary catalog route objects or a third reservation by accident. RA03 facts, waypoint placement and seam totals must be accumulated from the actual final emitted stream. Preview holds only final sealed route + bounded descriptor; Metadata commit uses the freed second reservation.

## R5 — Bound restart scope and define original-route lifetime

**Blocker. RA04 restart/retention; RA05 cancellation/rejoin.**

Current navigation is not persisted. Therefore “crash before checkpoint keeps previous accepted navigation” is false when that navigation was an ordinary nonpersisted route. General automatic route resume is substantial adjacent scope. An exact original lease keeps replaced bytes readable only during this process; after reboot the previous revision may not be a current catalog head. The derived visit itself can remain fully valid even if original restoration becomes unavailable. Current retention only protects active routes, and explicit replacement/removal is distinct from retention.

**Fix:** bound to recovery of the most recently accepted Assistant journey. On boot, offer Resume after validating the card and complete derived route; do not silently auto-resume every route. Before an initial Assistant checkpoint exists, a crash preserves ordinary boot behavior (it must not invent a recovered route). Manual normal-route activation or explicit navigation stop clears/supersedes the checkpoint before a durable success is reported; do not add persistence to all settings/normal routes.

Protect the referenced original route from automatic expiry while cancellation needs it. Decide explicit replacement/deletion: simplest is Busy/refusal while the active visit depends on that exact original, with ending/replacing the journey as the existing way to release it. Keep cleanup source-revision-specific. After rejoin, publish completed/no-visit checkpoint state, drop the original dependency, and allow future recovery to use the standalone derived route without requiring its historical original. A saved phase constrains initial matching; ambiguity must not choose the later overlapping leg. Recovery of recording remains a separate existing decision.

## R6 — Existing accepted blockage can be undone by an Assistant route

**Blocker. RA04/RA05/RA11 compatibility; epic promises not to remove working detour behavior.**

Current Road blocked/detour requests hold a temporary Corridor; committed ordinary route bytes do not preserve that forbidden span. A subsequent Easier route or visit return can legally choose the avoided road again. Leaving the old entry visible does not preserve the rider's explicit avoidance decision. Neither the draft request capture nor candidate eligibility mentions this.

**Fix:** add the smallest shared accepted-journey avoidance contract to the Navigator request stamp and route construction, or explicitly disable Assistant replanning when the active route has an unresolved avoidance it cannot preserve. Do not infer a blockage from a route name. If adding persisted spans, specify a fixed bound and map/source identity, retain existing Corridor semantics, and reject capacity instead of dropping earlier spans. Existing detour commit must set that metadata; this is compatibility work, not a new Road blocked screen. Test that Easier and visit return do not reintroduce an explicitly avoided span, including after recovery. Legacy routes without reliable provenance cannot claim they preserve unknown historical blockages.

## Nonblocking clarifications

### C1 — RA03/RA11 coverage and objective policy

“sufficient valid comparable elevation” needs a named minimum. For v1 use complete comparable elevation for a confident Less climbing saving and any ascent cap; partial values may be displayed with Unknown but cannot prove eligibility. Explain whether the common baseline trial is shared across goals and cap total trials explicitly (for example one baseline plus two per objective, at most seven). “Unsuitable” is not a separate graph boolean: retain existing forbidden classes; specify which remaining multipliers must not be neutralized for Shorter. The defined rough classes and no-growing-unknown rule are otherwise good.

### C2 — RA05 exact authored identity across a second replacement

Source route+stored ordinal identifies a waypoint in one revision. If a second derived route uses the first derived route as its new original, identity changes unless provenance is carried. Add durable lineage in the waypoint producer/transform contract or state a stable derived identity rule; do not rely on names (duplicates are legal). Keep coordinates/categories and new access placement distinct.

### C3 — Access metadata producer to consumer

RA02 preserves entrance/access links as build inputs; RA05 requires credible runtime approaches. State which producer resolves these to a bounded profile-compatible approach coordinate/edge association and which runtime field RA05 reads, including map revision and unavailable status. Avoid a plan where every fixture is info-only because no runtime approach was encoded. Require at least one real service and landmark with a successfully verified mapped visit.

### C4 — Scope restraint

RA11 explicitly permits unavailable during visits. Choose that conservative v1 behavior instead of implementing route changes inside an active excursion; other questions remain accessible. The user did not request improving an accepted visit in progress. Recovery can remain an explicit offer, and arrival notifications need not have exactly-once durable delivery. These choices reduce RA04/05 size without weakening accepted navigation safety.

## Readiness by reviewed child

- RA03: structurally ready; resolve C1 and coordinate byte owner with RA04/05.
- RA04: blocked by R1–R5 (and R6 shared avoidance stamp).
- RA05: blocked by R3–R6; C2/C3 clarify producer seams.
- RA11: blocked by inherited lifecycle/avoidance gaps; otherwise coherent, useful, bounded goals.
- RA13: sound delivery matrix; fix five-hold cap and reflect narrow explicit recovery semantics.
- Epic: dependencies coherent; do not publish ready until blockers are resolved. One delta review of changed sections is sufficient.
