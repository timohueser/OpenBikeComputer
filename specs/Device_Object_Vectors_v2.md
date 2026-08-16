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

The semantic body is the `body` object, and it is what makes a control fixture a *field* pin rather
than only a byte pin: three codecs can agree on every byte and still disagree about which field a
byte belongs to, and one that transposes two adjacent same-width fields re-encodes perfectly. It is
one flat object whose keys are field paths — `metadata.field[0].tag`, `entries[1].revision`,
`resourceLimits.routeHeads` — never a nested object, so an implementation in any language can build
the same map without a shared schema. Values follow the same rules as the rest of the file: JSON
numbers only for fields of at most 32 bits, canonical decimal strings for every `u64`/`i64`, and
lower-case hex for opaque byte fields including diagnostic text and metadata field values. An
enumerated field carries its wire number rather than a name, because a name is an implementation's
vocabulary rather than the contract's; a reserved field never appears, since a decoder proves it
zero and has nothing to report. A message with an empty payload carries `{}`. Each suite decodes the
frame with its own codec, rebuilds this map from the decoded values, and compares it to the fixture
in both directions: a fixture without a body, a body field the suite does not check, and a field the
suite reports that the fixture does not state are all failures.

A rejection fixture whose `target` is a bare metadata envelope also carries `class` — `put`, `patch`,
or `catalog` — and the `maximumEncodedLength` that class imposes. The class is the position the
envelope is being decoded in, which is what the wire contract's §2.2 makes the ceiling a fact about;
a suite takes the ceiling from that class and from its own constant for it, never from the version
byte the envelope carries, and an envelope that lies about its version is measured against the
ceiling of the position it actually occupies.

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
- InstallUpdate and AcknowledgeRideImported;
- GetDeviceStatus, GetConfig, SetConfig, SetClock, ForgetBond, Echo, and ResetStore.

Each emitted ObjectResult outcome, DraftPartResult, and AbortResult disposition has its own wire
vector; the reserved outcome `1` appears only as a decode-only row no device produces.
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
foreign/tampered ref, and child kind/key/length/CRC mismatch. The display-name field is pinned at
both boundaries: a full 32-byte name with no terminator, and a short name whose padding is zero,
against a rejection whose padding is not.

### 2.1 Exact-size goldens for the resumable acceptances

Three acceptances carry the finalized prefix CRC and are frozen at their exact sizes: UploadAccepted
at 64 bytes, DraftPartAccepted at 72, and the FinalizeDraft acceptance at 64. All three carry their
flags identically — a `u16` at offset 2, resumed-work bit 0 and restart-at-zero bit 1 — and one
negative per message places a flag in the byte at offset 1, which is reserved in two of them and
target mode in UploadAccepted, and MUST be rejected. Each has three
positive goldens — durable offset zero with CRC zero, a resume at a nonzero offset carrying the real
finalized CRC of exactly that prefix, and a restart-at-zero with the restart flag set — and one
negative twin encoded at the pre-freeze size (56, 68, and 56 respectively) that MUST fail decode
rather than decode short. A resume vector proves the comparison end to end: the client's retained
prefix CRC against the acceptance's field, on both the matching and mismatching branches.

### 2.2 Capability and paging vectors

Capability discovery is pinned in both directions: a ResourceLimits page whose byte 54 equals the
block's own byte 0 and whose byte 55 reports wire minor `0`, against a negative page where 54 and 0
disagree and which a client MUST reject without decoding either block; multi-page subject discovery
with the `more` flag set on every page but the last; a zero-subject device answering subject page
zero with count zero and `more` clear, with page index one rejected; and one transcript proving the
capability revision is constant across a whole discovery, since capabilities are immutable within a
connection generation and `catalogChanged/capabilitySnapshot` is reserved and never emitted. Frame-limit
derivation is
pinned as cases rather than prose: ATT MTU 247 yields a 244-byte ceiling, 195 yields exactly the
192-byte minimum, 194 is refused at Hello with `resourceLimit/minimumControlFrame`, and 66 produces
no frame at all because even the refusal is undeliverable. The stream side is pinned the same way at
CoC establishment: an SDU below the 64-byte floor refuses the channel with
`resourceLimit/minimumStreamFrame`, and an SDU below the negotiated stream maximum fixes the
effective limit at the SDU.

QueryCatalog paging splits the old maximum case in two, because the two bounds are independent: a
maximum-count page returning the most whole entries the largest frame carries — five ride entries at
429 payload bytes, since ten cannot fit any conforming frame — and a maximum-metadata
page at the 192-byte minimum. The metadata case's producible positive is a 162-byte-payload route
entry with `more` set, and the maximum StartUpload's is a 116-byte weather request. Neither ceiling
is a fixture: as the wire contract's §2.2 says, no legal envelope reaches one, so a ceiling vector
would be a fixture a conforming decoder must reject. The `44 + 36 + 96` and `48 + 128` ceilings are
asserted arithmetically instead. No page in
any vector exceeds the negotiated frame, and no entry is split across pages.

Both values of the StartUpload and StartDraftPart resume byte are pinned against both durable-work
states: the one resuming row, the two that discard durable work with restart-at-zero set, and the
two with no durable work and both flags clear, plus a rejection of a resume byte above `1`. No row produces `objectNotFound/resumableWork`, which is reserved. Checkpoint
sequencing is pinned too: the first durable checkpoint is `1`, sequences strictly increase, and a
resume continues the sequence rather than restarting it.

### 2.3 Device-control vectors

The device-control plane is frozen at its exact sizes: the 64-byte GetDeviceStatus response with a
mounted store and a second with no card at all — mount class `0`, StoreId zero, every other field
still populated; the 56-byte config block from GetConfig and the identical block returned by
SetConfig, with a full 32-byte name, an empty name, and a short name whose padding is zero; the
16-byte SetClock request and its 16-byte response for both sources; the 8-byte ForgetBond request
with an empty response; Echo at zero bytes, one byte, and the negotiated maximum; and the 16-byte
ResetStore request with its 16-byte response. The status response is pinned once per mount class
`0` through `6`, including the dynamic class `4` reached at a failed lazy pin and the store-wide
degraded class `6`, with the StoreId field zero except in classes `3`, `4`, and `6`. Negative
vectors cover a config block with a nonzero byte beyond the stated name length, a name length above
32, a weather-refresh value above `4`, a reserved unit-flag bit, a block length other than 56, an
unknown clock source, a companion SetClock moving a trusted clock backwards answered
`semanticValidation` in namespace zero with detail `clockRegression` — against a GPS-sourced set of
the same value that is accepted — a ResetStore whose echoed StoreId does not match and one carrying
the all-zero echo in a class that reports a StoreId, both
`invalidDescriptor/invalidCombination`, a ResetStore with no card answered
`mediaUnavailable/noCard` and one on an unsupported volume answered `mediaUnavailable/unmounted`,
and ForgetBond on a non-BLE link answered `unsupportedCapability/opcode`.
No device-control vector carries an OperationId, and no transcript shows one producing a retained
result.

## 3. Required stream vectors

Upload and download each include first, middle, and final frames; minimum one-byte payload; maximum
negotiated payload; nonzero absolute offset; and offsets around `0xFFFF_FFFF` without allocation of
the preceding bytes. A resumed-prefix CRC vector fixes CRC as the finalized CRC-32/IEEE of exactly
the durable prefix.

Fault status frames are pinned positively, not only as rejections: disposition
resume-with-a-new-session `0` in its nonterminal form (fault bit alone), and dispositions
operation-durably-aborted `1` and stream-transport-closed `2` in their terminal form (fault and
terminal bits together). Those three are the whole of the legal set; disposition `0` with the
terminal bit, and `1` or `2` without it, are negatives.

Negative stream vectors cover zero SessionId, wrong direction, zero payload, truncated/overlong
payload, reserved flags, `offset + length` overflow, wrong offset, stale same-link session,
cross-link session, wrong principal, and frames exceeding the negotiated limit. Each names the
required link/session action and subsequent QueryOperation result. Two session-lifetime transcripts
separate the cases that look alike: frames bearing a SessionId released earlier in this connection
generation are silently discarded with no fault and no transport close, while a SessionId never
issued in this generation closes the stream transport.

## 4. Required control rejections

At least one vector freezes every error category and allowed category/detail/retry combination.
The suite additionally includes:

- bad magic, incompatible major, unsupported minor/feature, zero RequestId, truncated frame,
  trailing bytes, length overflow, unknown opcode, and reserved header flags;
- malformed, duplicate, out-of-order, unknown critical, unknown mutating noncritical, wrong-kind,
  wrong-version, oversized, and noncanonical metadata envelopes;
- missing/invalid optional-presence combinations, zero SessionId, invalid cursor CRC, changed
  catalog/draft snapshot, and unusable negotiated frame limits;
- unauthenticated and unauthorized access for every access class, without protected existence,
  owner, revision, draft, or operation leakage;
- same OperationId/same intent replay and same OperationId/different intent conflict for each
  mutation family;
- pre-claim revision/resource rejection versus post-claim terminal semantic rejection;
- full work/draft/lease/catalog/result capacity and insufficient-space preflight, each in the
  category it is actually emitted under: an occupied ride slot as `busy/rideSlot` and a second
  BeginDraft while a parent is open as `busy/draftParents`, with `insufficientSpace/retainedPrevious`,
  `busy/retainedPrevious`, `busy/draftParts`, `resourceLimit/draftParents`,
  `resourceLimit/rideSlot`, `busy/maintenance`, `catalogChanged/capabilitySnapshot`,
  `objectNotFound/requestedRevision`, and
  `objectNotFound/resumableWork` appearing only as reserved decode-only rows that no device emits;
- all eight normal operation claims occupied while AbortOperation succeeds through the reserved
  cancellation/recovery claim;
- checksum, semantic, signature, digest, target, downgrade, power/runtime, recovery, and media-I/O
  outcomes;
- SetMetadata carrying a well-formed zero-field patch envelope, refused
  `invalidDescriptor/emptyMetadataPatch`;
- AbortOperation naming an InstallUpdate target, refused
  `unsupportedCapability/nonCancellableOperation` with guidance reject-permanently and both claim
  status bits clear;
- a subject entry with a nonzero patch schema version while its set-metadata flag is clear, and one
  with a patch schema version other than `128` while that flag is set;
- a StartDownload carrying the reserved revision flag or a nonzero reserved revision field, both
  `invalidDescriptor/reservedBits`;
- a zero-RequestId frame, which produces no response at all and closes the control record stream.

The two claim-status bits are pinned across the claim boundary in all three of their legal
combinations: both clear for every pre-claim class (version, framing, authentication, authorization,
descriptor, preflight, retryable domain precondition) and for `operationIdConflict` and
`mediaIo/uncertainCommit`, the latter with guidance query-OperationId-now; bit 5 set with bit 6 clear
for every error raised against a live claimed operation — `invalidOffset` on a live upload,
`checksumFailure/durablePrefix`, a mid-stream `mediaIo`, `invalidSession` against a live claim, and a
post-claim `busy` — and both set for every terminal report and for a retained-Aborted replay. Bit 6
set with bit 5 clear is a negative vector that MUST be rejected as a malformed body.

Retained-Aborted replay bodies are frozen for the categories whose live form has required presence —
`busy`, `invalidOffset`, `insufficientSpace`, and `mediaUnavailable` — proving the replay decodes
with those presence bits clear, owner none, guidance forced to reject-permanently `0`, and bits 5
and 6 both set. The same fixtures prove the decoder-side rule: no vector in this suite is rejected
for a present-but-unexpected or absent-but-expected optional field, because the presence matrix
binds senders only.

The owner byte is pinned at every value — none `0`, BLE `1`, USB `2`, test `3`, local producer `4`,
maintenance `5` — including one `busy` raised by a device-local producer against a link client, which
proves owner `4` and the link-kind values are read from one byte without being confused for the link
kind enum's own encoding.

Diagnostic text has its own boundary set, and one of them inverts the naive expectation: empty text,
text of exactly 64 bytes, and an invalid-UTF-8 text body that MUST still decode and be rendered
lossily rather than rejected. The genuine negatives are structural only: a text length above 64, and
a text length disagreeing with the frame's payload length, both `invalidFrame`.

Canonical intent is frozen as bytes, not as prose: for every mutating opcode in the canonical-suffix
table, a golden fixture carrying the exact 36-byte prefix, the exact suffix encoding, and the SHA-256
of the whole. A digest computed by a production encoder is not evidence; these fixtures are what a
same-intent replay and an `operationIdConflict` are judged against. The three device-local schemes —
`O2-LOCAL-WX-INTENT\0`, `O2-LOCAL-UPD-INTENT\0`, and `O2-LOCAL-IMP-INTENT\0` — are frozen the same
way as storage fixtures, proving that a local digest occupies the same field as a wire digest, that
lookup and comparison treat the two identically, and that the differing leading bytes make a
collision between the families impossible by construction.

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
    manifest validation, the resolution generation written before the publication gate, atomic final
    publication, parent abandonment, and child collection; a second BeginDraft while that parent is
    open refused `busy/draftParents` with no claim; initial volume-manifest publication has selected
    false, followed by explicit SetMetadata selection;
12. draft duplicate part key/role and foreign DraftPartRef rejection;
13. current weather request commit/satisfaction and weather deletion, proving that the weather head,
    the request-context pending/satisfied state, the repository Revision, and the queryable result
    all change at one publication boundary and that no observer sees any of them changed without the
    others — asserted from what a peer can observe (catalog, query, and result), since the commit
    notification is an in-process event with no wire form to freeze;
14. a bundle whose request context is no longer current rejected as `weather.requestMismatch`, with
    the current request left pending, the catalog head unchanged, and the compare-and-swap token
    still valid — proving the rejection is the context check and not a revision conflict;
15. active-ride append-journal checkpoint/recovery with an optional historical route snapshot,
    refusal or preservation when another recoverable ride occupies capacity, exactly-once
    immutable finalization, download, explicit imported acknowledgement, and acknowledgement
    replay;
16. update VerifiedReady without install, authenticated InstallUpdate at every catalog/handoff
    crash cut, reboot/trial/confirm, forced rollback, device-local state-operation recovery,
    catalog visibility, and QueryOperation truth;
17. media removal during streaming, checkpoint, validation, catalog commit body, commit gate, and
    recovery, always producing old-or-new visibility;
18. card replacement changes StoreId and rejects stale operation/session/catalog identities;
19. sideload import at mount: a staged route, a staged standalone map, and a staged update package
    each imported once and their staged files deleted; a staged file rejected by domain validation,
    leaving a terminal Aborted result, the file still on the card, and no retry within that mount,
    followed by a second mount that returns the retained Aborted result without re-reading the file;
    a staged file refused by capacity preflight, leaving no claim and being retried at the next
    mount; a crash cut between the publication gate and the staged-file deletion, whose next mount
    re-derives the same OperationId and intent digest, publishes nothing a second time, and deletes
    the file; an unknown staged name, which is ignored, never opened, and reported; more than eight
    staged files, of which exactly eight are imported in directory-entry order; and an imported
    update package that reaches VerifiedReady and installs only under a later explicit InstallUpdate;
20. device-control traffic interleaved with object work: GetDeviceStatus answered with no card
    present and again while a heavy transfer is streaming, SetConfig applied twice with identical
    blocks and identical responses and surviving a reboot between them, and a ForgetBond whose
    response is delivered before the bond is
    removed — none of them creating an operation, a result, or a catalog change;
21. ResetStore end to end: a mismatched StoreId echo refused with nothing destroyed, then an
    accepted reset whose response carries the new StoreId only after the first checkpoint gate is
    durable, with every earlier operation result, session, and lease gone, staged `IMPORT` files
    still present and imported by the new store at its first mount, and a stale
    operation/session/catalog identity from the old StoreId rejected afterwards.

## 6. Storage vectors and cut points

The storage suite includes empty and populated CAT checkpoints, every journal record variant,
active work slots, terminal ring wrap, multipart parent/child records, weather state, ActiveRideState
and every RIDE.ACT slot state, lease-free recovery projection, and both update-handoff slots. It
flips every byte/bit covered by a length, enum,
reserved field, complement, CRC, sequence, epoch, or validity gate and verifies deterministic
rejection. The reserved regions that must flip now include every slot's pad to its 16,384-byte
stride and the shrunk trailing zero runs of the retained-previous entry and the RIDE body.

It additionally includes:

1. **Slot strides.** A checkpoint, a journal slot, a WORK slot, a RIDE slot, and an ARM file at
   their exact sizes, proving the pad region is zero and that a decoder rejects a nonzero pad.
2. **Retained-generation reasons.** One entry per reason — live lease with a nonzero count, update
   rollback, and the weather domain-retention entry — plus an entry carrying two reasons at once, a
   full eight-entry table, and a rejection of a ninth. The weather pair is pinned as the two durable
   steps its replacement takes: the retention record clearing the older entry's reason bit, then the
   publication record retaining the newly displaced generation, with the cut between them leaving
   exactly zero or one domain-retention entry.
3. **RIDE.ACT observed length.** A recording slot whose observed payload length is below the durable
   offset, with the recovery outcome — rewind, re-verify the prefix CRC, resume — and the mismatch
   case that discards the ride payload.
4. **WORK observed-length rewind**, the same shape at the WORK body's observed-length field, since
   the mandatory rewind is new normative behaviour rather than a restatement.
5. **Removal keys, one vector per row type.** Catalog head, active operation, draft parent, draft
   part, retained previous, update handoff, and active ride, each with a negative twin carrying one
   nonzero non-key byte, and one with the occupied byte zero where the removal requires one.
6. **Key sort order.** Two 16-byte-keyed rows whose lexicographic wire-byte order differs from their
   little-endian integer order, proving the sort is lexicographic over wire bytes.
7. **All-slot journal scan.** A checkpoint at epoch `E` with a valid record at epoch `E+1` anywhere
   in the 256 slots, and a replay that stops at slot `k` while a valid same-epoch matching-StoreId
   record exists beyond it — both mounting recovery-failed rather than silently rolling back — against
   an ordinary end-of-journal image that mounts normally.
8. **Recovery-suffix budget.** The worst-case 55-record suffix — 32 draft-part transitions, 9
   active-row terminals, 4 update-reconciliation, 2 ride-publication, and 8 lease-clearing retention
   records — nine below the 64-slot headroom, plus the recovery-triggered compaction when fewer than
   64 slots are free.
9. **Durable lease clearing.** A reboot with a retained-previous entry carrying the live-lease bit,
   proving recovery appends and synchronizes the retention record clearing it before GC runs; and the
   negative twin where GC runs first, which is the permanently degraded outcome the rule exists to
   prevent.
10. **Manifest-ref binding and the resolution generation.** The resolution body at its exact sizes —
    one entry at 32 bytes and the 32-entry maximum at 776 — with its ordering, uniqueness, and count
    checks, against a truncated body whose count and length disagree, which makes the manifest's
    children conservatively reachable rather than orphaning them. Plus the two rejection cases
    finalization must produce: a manifest naming a `DraftPartRef` that matches no sealed row of this
    parent, and one naming a ref sealed under a different parent.
11. **Pre-birth prefixes.** An empty directory skeleton left by reset, which initialization reuses; a
    short or incomplete final file of the creation-order prefix, which is a bounded restart rather
    than corruption; and a prefix whose sorted order differs from its FAT physical directory-entry
    order, proving membership is judged over the physical order.
12. **Unsupported filesystem.** An exFAT image and a partitionless superfloppy image, plus two
    geometry failures — a FAT32 image with 4 KiB clusters, and one with 32 KiB clusters whose data
    region starts at a non-16,384-aligned physical offset (the classic LBA-63 partition start) —
    against a positive image whose 4 MiB-aligned partition and 32 KiB clusters satisfy both
    preconditions. All four failures mount unsupported with nothing written, distinct from
    fresh-card, recovery-failed, and degraded, and each names the computed check that rejected it.
13. **Lazy-pin degraded entry.** A mount that succeeds with a catalog entry whose generation file is
    missing, where the first pin reports that one entry degraded while the store stays writable and
    the mount is not degraded store-wide.
14. **Work-record trigger.** Sixty-three terminal commits with payload checkpoints producing no
    journal work record, and the sixty-fourth producing exactly one.
15. **Import staging.** A staged file of each importable kind with its derived local OperationId and
    intent digest computed from the frozen input bytes, including the content prefix digest over the
    first `min(4096, length)` bytes and the exact 85/89-byte inputs (87/91 for a map import's child,
    whose appended `DraftPartKind` is what separates it from its parent); two files sharing a name
    and a length but differing inside that prefix, proving they derive different identities; the
    synthesized map manifest with part key `1`, part kind `1`, and the display name taken from the
    FAT stem with trailing spaces stripped; an ignored unknown name; and the directory
    listing order the eight-per-mount bound is applied over.
16. **Storage-internal claim tags.** An active row carrying `0xFF01` and one carrying `0xFF02`,
    against rows carrying the wire opcodes a ride publication and each import kind store, and a
    rejection of a row whose opcode is neither a registered wire opcode nor a registered tag.
17. **Draft-part prepared state.** A part row in storage state `4` before its first accepted byte,
    its projection onto wire state `0`, and its transition to `aborted` without ever being
    `streaming`.
18. **Restart-at-zero durability.** A WORK pair recording a nonzero durable offset, the mandated
    reset slot written and synchronized before any payload byte is rewritten, and a cut placed
    between the reset slot's gate and the first new payload byte, which recovers at offset zero;
    against the negative ordering in which the payload is rewritten first and recovery aborts a
    healthy upload on a prefix-CRC mismatch.
19. **Lease retention across displacement.** Acquiring a lease writes no record at all; the
    publication that displaces the leased head retains that generation with the live-lease bit and
    the count of leases live at that moment; release appends the mirroring retention record that
    decrements it and removes the entry when no reason remains; and the count is exercised to the
    four-lease bound. Releasing a lease on a generation no entry names appends nothing.
20. **Unreasoned displacement.** A replace and a delete whose displaced generation carries no
    retention reason at all, proving the terminal record creates no entry and the generation is
    collectable at that gate.
21. **Compaction materialization.** A compaction pass whose card-resident per-head fields — the
    catalog-projection envelope and the resolution `GenerationId` — come from three sources at once:
    the RAM index, a journal-carried head entry located through the per-head journal-slot reference,
    and a copy from the active checkpoint. It produces byte-identical output
    to the same projection materialized in one piece, with the interrupted pass before the gate
    recovering the old checkpoint.

DraftPartRef fixtures treat the reference as 16 opaque bytes in every language, because that is all
it is: they freeze a sealed draft-part row holding its `(ref, GenerationId)` pair, the lookup that
resolves a manifest reference against that row, and the rejection of a reference matching no row.
Nothing decodes, derives, or recomputes a reference, and no fixture carries a key.

Crash enumeration cuts before and after every media write and sync in initialization, BeginWork,
payload checkpoint, seal, logical publication, terminal abort, checkpoint compaction, GC deletion,
BeginDraft/child seal/finalization, weather-context claim/publication/delete, every
RIDE.ACT/ActiveRideState start/checkpoint/stop/seal/claim/publish/discard transition, initial update
handoff, each post-boot update local claim, trial/outcome/complete ARM write, terminal state
commit, rollback-retention clear, and handoff removal, the resolution-generation reservation record
and the write that follows it, and each step of a staged import — claim,
copy, seal, publication gate, and staged-file deletion. Recovery must select the specified old, new,
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
