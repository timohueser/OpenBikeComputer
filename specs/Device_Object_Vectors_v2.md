# Device Object System v2 vector and transcript contract

Status: **normative inventory** for Device Object System v2. The checked-in fixture set lives under
`specs/vectors/device-object-v2/` and is incomplete until every required row below exists and passes
independent Rust, Swift, and TypeScript codecs.

Vectors freeze bytes; transcripts freeze state, ownership, durability, and retry meaning. A
production decoder must not generate its own expected bytes. The Rust fixture producer builds bytes
directly from the byte tables without calling the production encoder. Swift and TypeScript read the
same fixtures and independently decode and re-encode them.

## 1. Fixture format

`manifest.json` has `suite = "device-object-v2"`, `format = 1`, `wire_major = 3`, `storage_format =
1`, and arrays `controls`, `streams`, `storage`, `negative`, and `transcripts`. Every referenced
fixture has a unique stable name and SHA-256 of its canonical file bytes.

JSON represents `u64`/`i64` as canonical decimal strings and 16-byte opaque values as exactly 32
lower-case hexadecimal characters. Raw frames/records are lower-case even-length hexadecimal.
JSON numbers are used only for exactly representable fields of at most 32 bits. No fixture relies
on host UUID formatting, floating point, object-key order, or platform path separators.

A control fixture contains:

- name, direction, opcode name/value, header fields, semantic body, and exact frame hex;
- expected decoded type and byte-exact re-encoding;
- for errors, category/detail/retry/presence and whether the operation was durably claimed;
- applicable minimum/maximum boundary label.

A transcript is an ordered event list. Each event names actor/principal, link and connection
generation, request or stream bytes, injected disconnect/reset/cut, durable offset, visible catalog
revision/head, active session owner, QueryOperation state, terminal result, and expected response.
Storage-private IDs may appear only in storage fixtures, never client transcript state.

## 2. Required control vectors

For every request below the suite contains the smallest valid request, largest valid variable body
where applicable, successful response shape, every response disposition, and one reserved-bit or
reserved-byte rejection:

- Hello/Capabilities, including zero and maximum advertised kind capabilities;
- StartUpload, CheckpointUpload, FinishUpload;
- StartDownload and FinishDownload;
- AbortSession and AbortOperation;
- BeginDraft, StartDraftPart, FinalizeDraft, and QueryDraft;
- QueryOperation and QueryCatalog;
- QueryWeatherRequest;
- DeleteObject and SetMetadata;
- InstallUpdate and AcknowledgeRideImported.

Each OperationResult outcome, DraftPartResult, and AbortResult disposition has its own wire vector.
Each storage-local DomainResult outcome has a storage vector only; it is not part of the link
ResultEnvelope or the Swift/TypeScript wire API.
QueryOperation has Unknown, every normative opcode/phase/flag/subject combination, Committed, and
Aborted vectors. Retained-Aborted replay is covered for every start/resume mutation family.
Catalog vectors include empty, one-entry, maximum-count, maximum-metadata, unchanged, changed, final,
and continuing pages. Draft vectors include empty, prepared, streaming, sealed, aborted, final, and
continuing snapshot pages.

Every ObjectKind, DraftPartKind, metadata version, catalog projection, capability operation/policy
bit, retry guidance, common wire detail, and domain semantic detail appears in at least one
positive or rejection vector.

Volume-manifest vectors cover one standalone map and a maximum 32-part volume, plus bad magic,
version/length/count/order, duplicate kind/key or ref, missing/multiple core, invalid/outside bbox,
foreign/tampered ref, and child kind/key/length/CRC mismatch.

## 3. Required stream vectors

Upload and download each include first, middle, and final frames; minimum one-byte payload; maximum
negotiated payload; nonzero absolute offset; and offsets around `0xFFFF_FFFF` without allocation of
the preceding bytes. A resumed-prefix CRC vector fixes CRC as the finalized CRC-32/IEEE of exactly
the durable prefix.

Negative stream vectors cover zero SessionId, wrong direction, zero payload, truncated/overlong
payload, reserved flags, `offset + length` overflow, wrong offset, stale same-link session,
cross-link session, wrong principal, and frames exceeding the negotiated limit. Each names the
required link/session action and subsequent QueryOperation result.

## 4. Required control rejections

At least one vector freezes every error category and allowed category/detail/retry combination.
The suite additionally includes:

- bad magic, incompatible major, unsupported minor/feature, zero RequestId, truncated frame,
  trailing bytes, length overflow, unknown opcode, and reserved header flags;
- malformed, duplicate, out-of-order, unknown critical, unknown mutating noncritical, wrong-kind,
  wrong-version, oversized, and noncanonical metadata/extensions;
- missing/invalid optional-presence combinations, zero SessionId, invalid cursor CRC, changed
  catalog/draft snapshot, and unusable negotiated frame limits;
- unauthenticated and unauthorized access for every access class, without protected existence,
  owner, revision, draft, or operation leakage;
- same OperationId/same intent replay and same OperationId/different intent conflict for each
  mutation family;
- pre-claim revision/resource rejection versus post-claim terminal semantic rejection;
- full work/draft/lease/catalog/result capacity and insufficient-space preflight;
- all eight normal operation claims occupied while AbortOperation succeeds through the reserved
  cancellation/recovery claim;
- checksum, semantic, signature, digest, target, downgrade, power/runtime, recovery, and media-I/O
  outcomes.

## 5. Required semantic transcripts

The suite is not complete without all of these deterministic transcripts:

1. create upload, checkpoints, seal, publish, catalog query, and download;
2. replace admitted, concurrent mutation wins, publication CAS recheck rejects the stale replace;
3. committed terminal response lost, reconnect, QueryOperation returns the exact retained result,
   and retry creates no generation or second commit;
4. result-window fill and the 64th-newer-result eviction boundary, including Unknown reconciliation
   and no OperationId reuse;
5. disconnect after uncheckpointed bytes, reboot, resume from the last durable offset, and exactly
   one final publication;
6. AbortSession retains resumable work; AbortOperation durably abandons it and later collection is
   idempotent, including the ordered parent-aborting/child-terminal/parent-terminal/abort-result
   sequence and reset at every step;
7. stale same-link owner, wrong-link owner, wrong principal, delayed disconnect, repeated teardown,
   and attempted same-connection SessionId reuse cannot advance or release the current session;
8. download pin survives replace and delete, then becomes collectible only after matching release;
9. DeleteObject lost result/query and pinned-reader continuity;
10. SetMetadata compare-and-swap, lost result/query, and no sidecar state;
11. BeginDraft, out-of-order child parts, disconnect/reboot, snapshot paging, missing-part resume,
    manifest validation, atomic final publication, parent abandonment, and child collection;
    initial volume-manifest publication has selected false, followed by explicit SetMetadata
    selection;
12. draft duplicate part key/role and foreign DraftPartRef rejection;
13. current weather request commit/satisfaction and weather deletion, proving the weather head,
    request-context pending/satisfied state, repository Revision, CommitEvent, and results change
    at one publication boundary;
14. superseded weather bundle accepted with no current head or as strictly newer/useful while the
    newer request remains pending, plus equal/older/context-mismatch rejection;
15. active-ride append-journal checkpoint/recovery with an optional historical route snapshot,
    refusal or preservation when another recoverable ride occupies capacity, exactly-once
    immutable finalization, download, explicit imported acknowledgement, and acknowledgement
    replay;
16. update VerifiedReady without install, authenticated InstallUpdate at every catalog/handoff
    crash cut, reboot/trial/confirm, forced rollback, device-local state-operation recovery,
    catalog visibility, and QueryOperation truth;
17. media removal during streaming, checkpoint, validation, catalog commit body, commit gate, and
    recovery, always producing old-or-new visibility;
18. card replacement changes StoreId and rejects stale operation/session/catalog identities.

## 6. Storage vectors and cut points

The storage suite includes empty and populated CAT checkpoints, every journal record variant,
active work slots, terminal ring wrap, multipart parent/child records, weather state, ActiveRideState
and every RIDE.ACT slot state, lease-free recovery projection, and both update-handoff slots. It
flips every byte/bit covered by a length, enum,
reserved field, complement, CRC, sequence, epoch, or validity gate and verifies deterministic
rejection.

DraftPartRef fixtures freeze the private-key/context inputs, all six Feistel rounds, final opaque
bytes, inverse recovery of GenerationId, the 64-bit substitution-check boundary, and rejection
after parent/kind/key/length/CRC/reference tampering. Client fixtures treat the reference as bytes
and never decode it. The maximum-part vector records bounded decode cost for DOS2 resource review.

Crash enumeration cuts before and after every media write and sync in initialization, BeginWork,
payload checkpoint, seal, logical publication, terminal abort, checkpoint compaction, GC deletion,
BeginDraft/child seal/finalization, weather-context claim/publication/delete, every
RIDE.ACT/ActiveRideState start/checkpoint/stop/seal/claim/publish/discard transition, initial update
handoff, and each post-boot update local claim, trial/outcome/complete ARM write, terminal state
commit, rollback-retention clear, and handoff removal. Recovery must select the specified old, new,
or explicitly in-progress state, never a mixed catalog/result or actionable handoff without its
discoverable operation truth.

## 7. Cross-language acceptance

DOS1 codec acceptance requires:

- one dependency-light, allocation-free/no_std Rust production codec;
- a Rust spec-derived fixture producer independent of that codec;
- Swift and TypeScript codecs written from the normative tables, with no generated Rust bindings;
- byte-exact decode/re-encode parity for every positive fixture;
- identical typed rejection for every negative fixture;
- identical state/result observations for every transcript;
- checked-in fixture hashes and a CI guard that fails on an unreviewed fixture rewrite.

A domain opcode, schema, detail, state, or storage record is not frozen until its positive,
boundary, rejection, and lost-result behavior is represented here and in the checked-in manifest.
