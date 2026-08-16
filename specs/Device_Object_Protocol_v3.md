# Device Object Protocol v3 wire contract

Status: **normative** for the Device Object System v2 control and stream wire protocol. Its wire
major is **3** and minor is **0**. This is a clean cutover: a peer implementing this contract does
not translate or serve the legacy descriptor protocol.

This document owns framing, negotiation, transfer sessions, idempotency, result and error codecs,
and BLE/USB record binding. The [system contract](Device_Object_System_v2.md) owns identities and
ownership invariants, the [registries](Device_Object_Registries_v2.md) own object kinds and bounded
domain schemas, and the [storage format](OBC2_Storage_Format.md) owns persistence. Domain changes
may allocate semantic details and metadata fields only through the registry and shared vectors;
they may not change this common wire contract implicitly.

## 1. Representation and hard limits

`MUST`, `MUST NOT`, `SHOULD`, and `MAY` have their RFC 2119 meanings. Integers are unsigned
little-endian. Reserved fields and inactive fixed-width alternatives are encoded as zero and
rejected when nonzero. Sixteen-byte identities are opaque bytes copied without UUID field
reordering.

CRC is CRC-32/IEEE: reflected polynomial `0xEDB88320`, initial and final XOR `0xFFFFFFFF`, with
`crc32("123456789") == 0xCBF43926`. It detects accidental corruption; it is not identity,
authentication, authorization, or an idempotency proof.

| Limit | Value |
|---|---:|
| Minimum negotiated control frame, header included | 192 bytes |
| Maximum control frame, header included | 512 bytes |
| Minimum negotiated stream frame, header included | 64 bytes |
| Maximum stream frame, header included | 4096 bytes |
| Metadata envelope | 128 bytes |
| Catalog projection metadata | 96 bytes |
| Error diagnostic text | 64 UTF-8 bytes |
| Logical-kind and draft-part capabilities | 16 total |
| Retained terminal operation results | 64 |
| Default upload checkpoint granule | 262,144 bytes |

The negotiated limit is the smaller supported value advertised by the two peers. A value below a
protocol minimum fails Hello with `resourceLimit`; no reduced dialect exists. Values above a hard
maximum are `invalidFrame` before allocation. A kind's advertised maximum object length remains
authoritative even when a transport could carry more.

The 192-byte control minimum is derived, not padding: a maximum catalog entry is the 44-byte page
prefix plus a 36-byte entry prefix plus 96 metadata bytes, or 176 payload bytes and the 16-byte
control header. The largest mandatory v3.0 StartUpload descriptor (which has no defined extension)
is likewise 48 fixed bytes plus a 128-byte metadata envelope. The maximum text-bearing ErrorBody is
112 payload bytes. Implementations MUST recompute these maxima in shared codec tests when any
constituent limit or prefix changes.

The following 16-byte types are not interchangeable: `StoreId`, `OperationId`, and `DraftPartRef`.
A draft is identified by its parent `OperationId`; each part has a child `OperationId`. A
`DraftPartRef` is an authenticated opaque reference minted for a sealed part under exactly one
parent and resolved only after parent/principal authorization. It is neither a logical object
identity nor a physical generation identity.

## 2. Control frame

Every control transport record contains exactly one control frame with this 16-byte header:

| Offset | Size | Field | Rule |
|---:|---:|---|---|
| 0 | 4 | magic | ASCII `OBCP` (`4F 42 43 50`) |
| 4 | 1 | major | `3` |
| 5 | 1 | minor | `0` |
| 6 | 2 | opcode | Section 4 |
| 8 | 2 | flags | response bit 0, error bit 1, more bit 2 |
| 10 | 2 | payload length | exact bytes after this header, at most 496 |
| 12 | 4 | RequestId | nonzero; a response echoes its request |

Flags `3..15` are zero. Requests have no flags. Successful responses set `response`; errors set
`response|error`. `more` is valid only on a paged Capabilities, QueryCatalog, or QueryDraft
response. A client does not reuse a RequestId while its request is outstanding. There are no
unsolicited control frames.

`invalidFrame` means that a transport record cannot be established as one complete frame: bad
record length, truncation, trailing bytes, bad magic, payload-length mismatch/overflow, or a frame
outside negotiated bounds. If enough control header is trustworthy, the adapter returns an error;
otherwise it closes that record stream. `invalidDescriptor` means that a complete frame has an
illegal field value, reserved bit, enum, field combination, ordering, or nested length. An
unsupported parseable wire version is `incompatibleVersion`, not either malformed category.

### 2.1 Extension block

An operation explicitly described as extensible ends with zero or more fields:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 1 | tag |
| 1 | 1 | flags: mandatory bit 0 |
| 2 | 2 | value length |
| 4 | N | value |

Tags are strictly increasing and unique; flags `1..7` are zero. Unknown optional fields are
skipped and unknown mandatory fields are `unsupportedCapability`. Malformed, duplicate, or
out-of-order fields are `invalidDescriptor`. No v3.0 extension participates in mutation intent;
a future participating extension must define its canonical bytes and require a minor-version
feature before use.

### 2.2 Metadata envelope boundary

A metadata envelope starts with `schema_id u16`, `schema_version u8`, `flags u8`,
`encoded_field_bytes u16`, and `field_count u16`, followed by exactly the stated field bytes. Its
total length is therefore `8 + encoded_field_bytes`. Put and patch envelopes are at most 128 bytes,
so their encoded fields are at most 120 bytes. Catalog envelopes are at most 96 bytes, so their
encoded fields are at most 88 bytes. The header flags are zero.

Each field is `tag u16`, `value_length u16`, then exactly that many value bytes. The tag's high bit
is the critical bit and its low 15 bits are a nonzero base tag. Fields are strictly increasing by
base tag and base tags are unique; changing only the critical bit does not create another field.
`encoded_field_bytes` equals the exact sum of every `4 + value_length`, and `field_count` equals the
number of fields. Truncation, trailing bytes, padding, a zero base tag, duplicate/out-of-order base
tags, integer overflow, or a schema-disallowed width is `invalidDescriptor/noncanonicalMetadata`.

Schema integers use their exact registered width and little-endian encoding; signed values use
two's-complement at that width. Booleans are one byte and exactly `0` or `1`. Byte strings are
copied verbatim at their registered exact or bounded length. Text length counts encoded bytes.
Text MUST be shortest-form valid UTF-8 and contain no NUL, C0/C1 control, surrogate, or
noncharacter scalar. Noncharacters are `U+FDD0..U+FDEF` and every scalar whose low 16 bits are
`FFFE` or `FFFF`. Accepted bytes are canonical as-is: encoders and decoders MUST NOT normalize,
trim, case-fold, or otherwise rewrite them. Empty text is allowed only when its schema explicitly
permits it. Floating-point and platform-sized numeric fields are not metadata wire types.

The registries assign each field's exact critical bit, type, width/range, and required/optional
status. Schema ID exactly matches the containing logical ObjectKind, schema version is the one
advertised for that operation/projection, and every registered required field appears exactly once.
Even a schema with no fields uses its canonical eight-byte header with both counts zero. Mutating
requests reject every unknown field, whether critical or not. Response decoders
reject unknown critical fields and may skip a well-formed unknown noncritical field. This header
and the exact field sum give a decoder an unambiguous extension-block boundary; silently ignoring a
requested mutation is forbidden.

Shared vectors include empty envelopes, every registered scalar/string form, critical and
noncritical unknowns, duplicate/out-of-order base tags, malformed UTF-8/forbidden scalars, exact
120/88-byte field-body maxima, and one-byte-over failures. Codec tests also assert the 176-byte
maximum StartUpload and maximum one-entry catalog payloads.

## 3. Authentication, principals, and ownership

The transport adapter establishes a stable authenticated principal scope and a connection
generation. BLE derives the scope from the authenticated application/bond identity. USB uses an
authenticated application principal; cable possession alone is not authentication. A locally
entered developer/unlocked mode may establish a distinct local-development principal and is
reported by Capabilities. It cannot be enabled remotely.

Authorization is per opcode and, where applicable, per ObjectKind. Capability advertisement is
not authorization. The minimum matrix is:

| Operation | Required authority |
|---|---|
| Hello/Capabilities | may be unauthenticated; protected facts may be suppressed |
| CheckpointUpload, FinishUpload, FinishDownload, AbortSession, stream data | exact current SessionId owner |
| QueryOperation, QueryDraft, AbortOperation | authenticated owner of the operation/draft |
| QueryCatalog, QueryWeatherRequest, downloads | authenticated domain read authority |
| uploads, BeginDraft, FinalizeDraft, DeleteObject, SetMetadata | authenticated domain write authority |
| InstallUpdate | authenticated update-install authority |
| AcknowledgeRideImported | authenticated ride-write authority |

Authentication and authorization precede object-existence, revision, operation-status, and busy
facts. An `OperationId` claim stores an opaque stable principal-scope digest. Reconnect by the same
principal may resume/query it. A different principal receives `authorizationFailed`, not status or
`operationIdConflict`. Local producers use their own principal scopes and cannot be impersonated by
a link.

A SessionId is valid only with its link kind, principal scope, and connection generation. A
reconnect makes every earlier SessionId stale even for the same principal. Wrong-owner stream,
finish, checkpoint, or disconnect handling cannot advance or release a current session.
Within one connection generation, an adapter never issues the same nonzero SessionId twice,
including after its earlier session terminates. It maintains a monotonically advancing allocator or
an equivalent used-set; it closes and reconnects before the nonzero `u32` space would be exhausted.
Numeric reuse is permitted only in a new connection generation, where the generation owner check
makes every old capability stale.

## 4. Operation registry

Requests and successful responses share an opcode; the response flag distinguishes direction.

| Opcode | Operation | Mutation/claim |
|---:|---|---|
| `0x0001` | Hello / Capabilities | no |
| `0x0100` | StartUpload / UploadAccepted | durable logical Put claim |
| `0x0101` | CheckpointUpload | no new claim |
| `0x0102` | FinishUpload | completes upload claim |
| `0x0110` | StartDownload / DownloadAccepted | no |
| `0x0111` | FinishDownload | no |
| `0x0120` | AbortSession | session teardown only |
| `0x0130` | BeginDraft | yes |
| `0x0131` | StartDraftPart / DraftPartAccepted | durable child claim |
| `0x0132` | FinalizeDraft | completes parent claim |
| `0x0200` | QueryOperation | no |
| `0x0201` | QueryCatalog | no |
| `0x0202` | QueryDraft | no |
| `0x0203` | QueryWeatherRequest | no |
| `0x0300` | DeleteObject | yes |
| `0x0301` | SetMetadata | yes |
| `0x0302` | AbortOperation | yes |
| `0x0310` | InstallUpdate | yes |
| `0x0311` | AcknowledgeRideImported | yes |

Unknown opcodes are `unsupportedCapability`. Unsupported known operations are likewise rejected;
they are never forwarded through a generic repository facade.

## 5. Hello and complete capability discovery

Hello is 12 bytes:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 1 | minimum wire major |
| 1 | 1 | maximum wire major |
| 2 | 2 | client maximum control frame |
| 4 | 2 | client maximum stream frame |
| 6 | 4 | client feature flags; zero in v3.0 |
| 10 | 1 | page kind: resource limits `0`, subject capabilities `1` |
| 11 | 1 | zero-based page index |

Capabilities has this 56-byte common prefix followed by either one 56-byte ResourceLimits body or
up to two complete 20-byte subject entries:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 1 | selected wire major, `3` |
| 1 | 1 | OBC2 storage format version, `1` |
| 2 | 2 | status flags |
| 4 | 16 | StoreId, zero only when store-available is clear |
| 20 | 2 | negotiated maximum control frame |
| 22 | 2 | negotiated maximum stream frame |
| 24 | 4 | durable upload checkpoint granule |
| 28 | 2 | retained result capacity, exactly `64` |
| 30 | 2 | metadata envelope limit, `128` |
| 32 | 2 | catalog metadata limit, `96` |
| 34 | 2 | protocol minimum control frame, `192` |
| 36 | 2 | protocol minimum stream frame, `64` |
| 38 | 1 | link kind: BLE `1`, USB `2`, test `3` |
| 39 | 1 | auth state: unauthenticated `0`, authenticated `1` |
| 40 | 4 | capability revision |
| 44 | 4 | command flags |
| 48 | 2 | total subject count, at most 16 |
| 50 | 1 | returned page kind |
| 51 | 1 | returned page index |
| 52 | 1 | returned subject count, zero on resource page |
| 53 | 1 | total pages of this kind |
| 54 | 1 | ResourceLimits codec version, `1` |
| 55 | 1 | reserved |

Status flags are store available bit 0, authenticated bit 1, heavy-transfer busy bit 2, and
developer/unlocked mode bit 3. Command flags advertise QueryOperation bit 0, QueryCatalog bit 1,
QueryDraft bit 2, QueryWeatherRequest bit 3, BeginDraft bit 4, StartDraftPart bit 5, FinalizeDraft
bit 6, AbortOperation bit 7, InstallUpdate bit 8, and AcknowledgeRideImported bit 9. Other bits are
zero.

Each subject entry is exactly 20 bytes: namespace `u8` (logical ObjectKind `1`, DraftPartKind `2`),
reserved `u8`, kind code `u16`, operation flags `u16`, policy flags `u16`, Put schema version `u8`,
patch schema version `u8`, catalog schema version `u8`, reserved `u8`, and maximum length `u64`.
Operation flags are put bit 0, get bit 1, delete bit 2, set-metadata bit 3, resumable upload bit 4,
resumable download bit 5, and draft-finalize bit 6. Draft-part subjects advertise put and optional
resumable upload only; all three schema versions are zero because StartDraftPart has no metadata
envelope or catalog. Policy flags are USB recommended bit 0,
external power required bit 1, authenticated
principal required bit 2, and fixed singleton bit 3. Other bits are zero.

Page kind `0` has only index zero and returns the ResourceLimits block in Section 5.1. Page kind `1`
uses `first_subject = page_index * 2`, returns up to two entries in ascending
`(namespace, kind_code)` order, and sets `more` when another subject page exists. Total pages is one
for resources and `ceil(total_subject_count / 2)` for subjects. Capability revision identifies the
snapshot used for a page; a client requires the same value across discovery and restarts at the
resource page if it changes. The server increments it before a static subject or fixed resource
limit changes; ephemeral status flags and currently available reservation bytes are snapshots and
do not churn this revision. A StoreId change tears down the connection. Capability revision is
monotonic within a connection and the adapter reconnects before it wraps. A nonzero resource-page
index or an index beyond the last subject page is `invalidDescriptor`. The server never silently
truncates the registry. At the minimum 192-byte frame, the 176-byte payload easily holds either the
common prefix plus ResourceLimits (112 bytes) or the prefix plus two subject entries (96 bytes).

The resource page sets `more` when subjects exist; a subject page sets it when another subject page
exists. Discovery is complete only after resource page zero and every subject page have been read
under one capability revision.

### 5.1 ResourceLimits

The resource page reports fixed product/storage capacities plus one current reservation-space
snapshot. Admission still reports authoritative required/available values in ErrorBody. This lets
a client avoid a workflow the device can never accommodate without treating discovery as a lease.
The block is exactly 56 bytes; fixed values below are normative OBC2 v1 constants and a
format/resource review is required to raise them.

| Offset | Size | Field | Value |
|---:|---:|---|---:|
| 0 | 1 | codec version | 1 |
| 1 | 1 | block length | 56 |
| 2 | 2 | flags | 0 |
| 4 | 2 | logical catalog heads across all kinds | 256 |
| 6 | 1 | normal active claimed operations | 8 |
| 7 | 1 | resumable upload/work slots | 4 |
| 8 | 1 | active draft parents | 2 |
| 9 | 1 | sealed/streaming draft parts across all parents | 32 |
| 10 | 1 | children referenced by one manifest | 32 |
| 11 | 1 | simultaneously mounted map data files | 11 |
| 12 | 1 | live reader leases | 4 |
| 13 | 1 | retained previous generations | 16 |
| 14 | 2 | retained terminal results | 64 |
| 16 | 2 | inactive-work horizon in later terminal commits | 256 |
| 18 | 2 | journal slots | 256 |
| 20 | 8 | maximum embedded-FAT generation length | `0x0000_0000_FFFF_FFFF` |
| 28 | 8 | currently available reservation bytes | dynamic |
| 36 | 2 | route catalog heads | 64 |
| 38 | 2 | trip catalog heads | 16 |
| 40 | 2 | ride catalog heads | 128 |
| 42 | 2 | weather catalog heads | 1 |
| 44 | 2 | volume-manifest catalog heads | 8 |
| 46 | 2 | update-package catalog heads | 8 |
| 48 | 1 | simultaneously attached heavy stream sessions | 1 |
| 49 | 1 | reserved maintenance/cancellation/recovery claims | 1 |
| 50 | 1 | active-or-recoverable ride slots | 1 |
| 51 | 5 | reserved | zero |

The one-heavy-transfer coordinator allows one simultaneous streaming session. The four work slots
are durable resumable upload records, not concurrent streams. A device that omits an optional kind
still reports its fixed storage partition limit here; the subject registry is the authority for
whether that kind is usable.

Normal requests cannot consume the maintenance/cancellation/recovery claim. When all eight normal claims are
occupied, a new AbortOperation may claim the reserved slot and unwind one target. Recovery uses the
same slot with priority. If that slot is occupied, a different new AbortOperation returns
`busy/maintenanceCancellationRecoveryClaim` with owner `maintenance` and guidance `owner release`; it does not
partially cancel or claim a normal slot. A retry of the OperationId already owning the reserved slot
resumes it. The reserved claim is released only after its AbortResult or recovery result is durable.

## 6. Upload and draft protocols

Only one heavy transfer may own the device transfer coordinator. Resource limits are checked after
authorization and idempotency lookup but before storage allocation.

### 6.1 StartUpload

StartUpload is only a logical-object Put. It has a fixed 48-byte prefix, one metadata envelope,
then extensions:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 16 | OperationId |
| 16 | 2 | ObjectKind |
| 18 | 1 | target mode: create `0`, replace `1` |
| 19 | 1 | resume: forbid `0`, prefer `1`, require `2` |
| 20 | 8 | LogicalObjectId |
| 28 | 8 | expected object Revision |
| 36 | 8 | declared length |
| 44 | 4 | expected whole-object CRC |
| 48 | 8..128 | exactly one metadata envelope |

Create encodes logical ID and expected revision as zero. Replace requires the logical identity
supplied by the repository and its exact expected revision. Other combinations are
`invalidDescriptor`. The minimum request payload is 56 bytes and the maximum is 176 bytes,
including its envelope.

For a fixed-singleton kind, store initialization reserves one stable LogicalObjectId whether or not
a head exists. StartUpload always uses replace mode with that identity and the repository's current
Revision; it may create the first head as the singleton-specific replace semantic. Create mode and
any other identity are rejected. The weather singleton is not identified by `WeatherRequestId`,
zero, or a sentinel.

UploadAccepted disposition `0` is exactly 56 bytes:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 1 | disposition, accepted `0` |
| 1 | 1 | target mode |
| 2 | 2 | resumed-work bit 0, restart-at-zero bit 1 |
| 4 | 16 | OperationId |
| 20 | 4 | fresh SessionId |
| 24 | 8 | assigned/named LogicalObjectId |
| 32 | 8 | repository Revision observed at admission |
| 40 | 8 | authoritative durable next offset |
| 48 | 4 | checkpoint granule |
| 52 | 2 | maximum stream payload |
| 54 | 2 | reserved |

Disposition already terminal `1` is `u8 disposition`, three reserved bytes, then the typed
ResultEnvelope from Section 10. A same-intent in-progress operation receives a fresh SessionId
bound to the current connection and the same reservation/work. A retained Aborted operation does
not use a disposition: it returns a `response|error` control frame containing exactly its bare
48-byte, text-free terminal ErrorBody. No stream session is created for either terminal replay.

### 6.2 CheckpointUpload and finalized-prefix CRC

CheckpointUpload request is `SessionId u32` and received next offset `u64`, exactly 12 bytes. The
offset equals the in-memory next offset and a checkpoint granule, except at declared end. The
20-byte response is `SessionId u32`, durable next offset `u64`, finalized prefix CRC `u32`, and
checkpoint sequence `u32`.

The prefix CRC is ordinary finalized CRC-32/IEEE over exactly bytes `[0, durable_next_offset)`,
including the final XOR. A resume implementation may invert that final XOR to restore its rolling
state. The response is emitted only after payload bytes and the work record containing offset,
finalized CRC, and sequence are durable. A resumed client that retained prefix bytes MUST compare
their finalized CRC before sending new bytes; mismatch requires restart at zero or AbortOperation,
never concatenation onto an unverified prefix.

### 6.3 FinishUpload

FinishUpload request is exactly `SessionId u32`. The engine verifies length and whole-object CRC,
seals the bytes, and invokes the typed validator. Under the store commit lock it rechecks singleton
reservation or expected revision immediately before publication. Conflict leaves the old logical
head unchanged and durably aborts the operation. A logical StartUpload or parent-manifest session
returns ObjectResult wrapped in ResultEnvelope. A StartDraftPart session returns DraftPartResult.

Before publication, failures produce a terminal aborted result. Publication and terminal success
are one durable commit. A lost response therefore means unknown delivery, not failed mutation; the
client queries its OperationId.

### 6.4 AbortSession and AbortOperation

AbortSession request is `SessionId u32`, reason `u8`, and three zero bytes. Reasons are
client-cancelled `1`, request-superseded `2`, and user-requested `3`. Response byte `0` means the
session was detached and `1` means it was already terminal. Stale/wrong owner is `invalidSession`
and changes nothing. Detaching a resumable upload preserves its durable work; detaching a
restart-only upload durably aborts it.

AbortOperation is the explicit persistent cancellation command. Its 40-byte request is:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 16 | new OperationId for this abort command |
| 16 | 16 | target OperationId (parent or ordinary operation) |
| 32 | 1 | reason: client-cancelled `1`, superseded `2`, user-requested `3` |
| 33 | 7 | reserved |

It requires the target's owning principal. A draft-parent cancellation has this exact durable
sequence:

1. Claim the abort command in the cancellation/recovery slot and commit the parent state
   `aborting`; this rejects every new child and FinalizeDraft.
2. In deterministic `(DraftPartKind, part_key)` order, durably terminal-Abort each nonterminal
   child and release its work. Already sealed/terminal child results are unchanged.
3. Durably terminal-Abort the target parent with its text-free cancellation ErrorBody and remove
   the draft parent row.
4. Durably commit the abort command's AbortResult, then release the reserved claim and respond.

Each transition is bounded and recovery resumes at the first incomplete step after any cut. The
target parent never receives an AbortResult; that typed success belongs only to the separate abort
command. For a non-parent target, the durable sequence is claim the abort command, mark the target
terminal Aborted, then commit the abort command's AbortResult. If the target was already terminal, it is unchanged and the
abort result says `already terminal`. No success response is sent before the abort command result
is durable. Repeating the abort command is idempotent by its own OperationId.

### 6.5 BeginDraft, StartDraftPart, and FinalizeDraft

BeginDraft creates bounded multipart work and claims the future logical publication under its
parent OperationId. Its request is exactly 52 bytes:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 16 | parent OperationId |
| 16 | 2 | final ObjectKind |
| 18 | 1 | target mode: create `0`, replace `1` |
| 19 | 1 | reserved |
| 20 | 8 | LogicalObjectId, zero for create |
| 28 | 8 | expected Revision, zero for create |
| 36 | 8 | declared final manifest length |
| 44 | 4 | declared final manifest CRC |
| 48 | 2 | exact expected part count |
| 50 | 2 | reserved |

Only a kind whose operation flags permit draft finalization is valid; v3.0 uses volume manifest.
The exact part count is nonzero and no greater than the advertised maximum. Target field rules
match StartUpload. BeginDraft disposition `0` is a four-byte disposition/reserved prefix followed
by exactly 28 bytes: parent OperationId `[16]`, draft revision `u64`, expected parts `u16`, state `u8`
(open `0`), and reserved `u8`. Disposition already terminal `1` is the same prefix followed by the
parent's ObjectResult envelope. BeginDraft remains InProgress after disposition `0`; it does not
consume a terminal-result slot until finalization or abort.

A retained Aborted parent is replayed as `response|error` plus its bare text-free ErrorBody, not as
a BeginDraft disposition.

StartDraftPart is exactly 64 bytes:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 16 | child OperationId |
| 16 | 16 | parent OperationId |
| 32 | 2 | DraftPartKind |
| 34 | 2 | reserved |
| 36 | 8 | part key |
| 44 | 8 | declared length |
| 52 | 4 | expected CRC |
| 56 | 1 | resume: forbid `0`, prefer `1`, require `2` |
| 57 | 7 | reserved |

The child OperationId must be distinct from the parent and every other child. The part kind must be
advertised. `(DraftPartKind, part_key)` is unique within the parent. DraftPartAccepted disposition `0` is
exactly 68 bytes: disposition `u8`, flags `u8` (resumed bit 0, restart-at-zero bit 1), reserved
`u16`, child OperationId `[16]`, parent OperationId `[16]`, SessionId `u32`, DraftPartKind `u16`, reserved
`u16`, part key `u64`, durable next offset `u64`, checkpoint granule `u32`, maximum stream payload
`u16`, and reserved `u16`. Disposition `1` is the common four-byte disposition prefix followed by
the retained DraftPartResult envelope.

A retained Aborted child is replayed as `response|error` plus its bare text-free ErrorBody. The
accepted response contains no DraftPartRef: that authenticated opaque reference does not exist
until sealing is durable and appears only in DraftPartResult and QueryDraft's sealed entry.

FinalizeDraft request is exactly the parent OperationId `[16]`. It does not claim a new operation
or add semantic intent; BeginDraft already bound target, expected revision, manifest length/CRC,
and exact child count. After every declared child is sealed, disposition `0` returns this 56-byte
parent-manifest acceptance:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 1 | disposition, accepted `0` |
| 1 | 1 | resumed-work bit 0, restart-at-zero bit 1 |
| 2 | 2 | reserved |
| 4 | 16 | parent OperationId |
| 20 | 4 | fresh SessionId |
| 24 | 8 | assigned/named LogicalObjectId |
| 32 | 8 | repository Revision observed at admission |
| 40 | 8 | authoritative durable manifest offset |
| 48 | 4 | checkpoint granule |
| 52 | 2 | maximum stream payload |
| 54 | 2 | reserved |

Disposition already terminal `1` is the common four-byte disposition prefix followed by the
parent ObjectResult envelope. Missing/nonsealed children return a retryable domain-state error and
create no session. FinishUpload verifies the manifest against BeginDraft's declared length/CRC,
resolves every opaque child ref against the same parent, requires exactly the declared number of
unique `(DraftPartKind, part_key)` entries, and rejects missing, foreign, repeated, or unsealed
refs. Publication makes the logical manifest and all referenced parts reachable atomically. It
returns the parent's ordinary committed ObjectResult; no physical GenerationId is exposed. The
expected revision from BeginDraft is rechecked under the commit lock.

A retained Aborted parent is replayed from FinalizeDraft as `response|error` plus its bare
text-free ErrorBody, with no disposition or session.

## 7. Downloads

StartDownload request is 28 bytes: `ObjectKind u16`, flags `u16` (requested revision bit 0, start
offset bit 1), `LogicalObjectId u64`, requested object Revision `u64`, and start offset `u64`.
Inactive fields are zero. When revision is present, the repository must pin that exact immutable
revision, including an authorized retained historical revision; it MUST NOT silently substitute
the current head. An unavailable requested revision is `objectNotFound`. A nonzero start offset is
allowed only when the kind advertises resumable download.

DownloadAccepted is exactly 60 bytes: `StoreId[16]`, `SessionId u32`, `LogicalObjectId u64`, pinned
Revision `u64`, total length `u64`, whole-source CRC `u32`, accepted start offset `u64`, maximum
stream payload `u16`, and reserved `u16`. Resolve and lease occur before this response. Replace or
delete changes visibility but not the pinned bytes.

FinishDownload request is exactly 16 bytes: `SessionId u32`, received whole-source length `u64`,
and whole-source CRC `u32`. Length and CRC include a locally retained prefix when start offset was
nonzero. Successful empty response releases the lease exactly once. A malformed finish retains the
session until matching abort or disconnect so it cannot release another reader's lease.

## 8. Queries

### 8.1 QueryOperation and the 64-result bound

Request is one OperationId. The response begins with state `u8` and three reserved bytes:

| State | Value | Remaining bytes |
|---|---:|---|
| Unknown | 0 | none |
| InProgress | 1 | 24-byte progress body |
| Committed | 2 | ResultEnvelope |
| Aborted | 3 | ErrorBody without diagnostic text |

The 24-byte progress body is subject namespace `u8`, phase `u8`, flags `u8`, reserved `u8`, subject
kind `u16`, reserved `u16`, assigned LogicalObjectId `u64`, and durable offset `u64`. Namespaces are
none `0`, logical ObjectKind `1`, and DraftPartKind `2`. Phases are prepared `0`, streaming `1`,
sealed `2`, validating `3`, ready-to-publish `4`, reconciling-external-handoff `5`, draft-open `6`,
and aborting `7`. Flags are resumable bit 0, session-currently-attached bit 1, and
logical-ID-present bit 2; bits `3..7` are zero. Attachment is advisory and grants no ownership.

The originating claim fixes every progress field according to this matrix. A phase outside its row,
a nonzero kind in namespace none, or a nonzero ID/offset where the matrix says zero is an internal
state/codec error and MUST NOT be emitted.

| Originating claim | Namespace and kind | Allowed phases | Flags, ID, and durable offset |
|---|---|---|---|
| StartUpload `0x0100` | logical `1`, request ObjectKind | `0..4`, aborting `7` | resumable reflects claimed policy; attached only while that session exists; ID-present set; ID is assigned/named object; offset is durable payload prefix, declared length in phases `2..4`; aborting has no attachment |
| StartDraftPart `0x0131` | draft part `2`, request DraftPartKind | `0..4`, aborting `7` | resumable/attached as above; ID-present clear and ID zero; offset is durable part prefix, declared length in phases `2..4`; aborting has no attachment |
| BeginDraft/FinalizeDraft parent `0x0130` | logical `1`, final ObjectKind | draft-open `6`, `0..4`, aborting `7` | ID-present set; draft-open/aborting have offset zero and no attached session; manifest phases use resumable/attached and durable manifest offset |
| DeleteObject `0x0300` | logical `1`, request ObjectKind | `3`, `4`, aborting `7` | only ID-present set; ID is target; offset zero |
| SetMetadata `0x0301` | logical `1`, request ObjectKind | `3`, `4`, aborting `7` | only ID-present set; ID is target; offset zero |
| AbortOperation command `0x0302` | none `0`, kind zero | aborting `7` | flags, ID, and offset zero |
| InstallUpdate `0x0310` | logical `1`, update `7` | `3`, `4`, `5` | only ID-present set; ID is package; offset zero |
| AcknowledgeRideImported `0x0311` | logical `1`, ride `3` | `3`, `4`, aborting `7` | only ID-present set; ID is ride; offset zero |

An ID field with ID-present clear is zero. With the bit set, zero remains a valid opaque
LogicalObjectId. A terminal claim is never reported InProgress.

The store retains exactly the latest 64 terminal committed or durably aborted operations in
store-global commit order; active work does not occupy those slots. Unknown means only that the ID
is neither active nor retained. It cannot distinguish never claimed from evicted. A client must
settle uncertainty before 64 later terminal operations complete. After possible eviction it MUST
NOT replay the old OperationId or issue the same intended mutation under a new ID without an
independent domain-state reconciliation.

### 8.2 QueryCatalog

Request is 28 bytes: `ObjectKind u16`, flags `u16`, expected repository Revision `u64`, and cursor
`[16]`. Flags are expected-revision bit 0 and cursor bit 1. With neither flag, both fields are zero
and the current first page is requested. Expected-revision alone is an incremental unchanged
check: an exact match returns the response prefix with zero entries, a zero cursor, and no `more`
flag even when the catalog is nonempty. A mismatch is `catalogChanged` with current Revision; the
client then requests a current first page without the expected-revision flag. Cursor requires both
bits and an expected revision equal to the cursor revision. Other combinations are
`invalidDescriptor`.

Cursor bytes are repository Revision `u64`, next entry index `u16`, ObjectKind `u16`, and CRC-32
over current StoreId followed by those first 12 cursor bytes. They are opaque to application code
despite their normative codec. A revision mismatch is `catalogChanged` with current-revision
presence and retry guidance refresh.

The 44-byte response prefix is `StoreId[16]`, ObjectKind `u16`, entry count `u16`, repository
Revision `u64`, and next cursor `[16]`. The next cursor is zero unless `more` is set. Each 36-byte
entry prefix is `LogicalObjectId u64`, object Revision `u64`, length `u64`, CRC `u32`, flags `u16`,
metadata length `u16`, and reserved `u32`, followed by that many metadata bytes. Metadata length is
`8 + encoded_field_bytes` and at most 96. Entries are ordered by LogicalObjectId. At most ten whole
entries are returned and at least one when one remains; a schema that cannot fit one entry is an
internal contract error. One maximum entry makes a `44 + 36 + 96 = 176` byte payload and therefore
fits the 192-byte control minimum exactly. Entry flags are zero in v3.0 and nonzero values are
rejected.

### 8.3 QueryDraft

QueryDraft request is exactly 44 bytes:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 16 | parent OperationId |
| 16 | 2 | flags: expected revision bit 0, cursor bit 1 |
| 18 | 1 | requested limit, 1 through 6 |
| 19 | 1 | reserved |
| 20 | 8 | expected draft revision |
| 28 | 16 | cursor |

With neither flag, both fields are zero and the current first page is requested. Expected-revision
alone requires an exact match and then returns that snapshot's first page; a mismatch is
`catalogChanged`. Cursor requires both flags and the same expected revision. Other combinations are
`invalidDescriptor`. Cursor is draft revision `u64`, next entry index `u16`, zero `u16`, and CRC-32
over current StoreId, parent OperationId, then those first 12 cursor bytes. This
binds a cursor to one store and parent. Draft revision increments for every child-state change, abort, or seal. A
mismatch is `catalogChanged`. This gives each page one stable snapshot; clients restart from page
zero after a change.

QueryDraft is defined only while the parent claim is InProgress. The terminal commit removes the
draft-parent row; no finalized/aborted row is retained for paging. When the parent has a retained
terminal result, QueryDraft returns `objectNotFound/operationTerminal` with guidance
`query OperationId now`; the client uses QueryOperation for the ObjectResult or terminal ErrorBody.
When neither an active parent nor retained result exists, it returns
`objectNotFound/draftParentUnknown`. This distinction is emitted only after principal ownership is
authorized.

The 44-byte response prefix is parent OperationId `[16]`, draft revision `u64`, next cursor `[16]`,
count `u8`, flags `u8` (manifest-streaming bit 0, aborting bit 1), and reserved `u16`. Up to six 68-byte
entries follow:
child OperationId `[16]`, DraftPartRef `[16]`, DraftPartKind `u16`, reserved `u16`, part key `u64`, state
`u8`, flags `u8`, reserved `u16`, durable offset `u64`, declared length `u64`, and CRC `u32`.
DraftPartRef is zero unless state sealed `2`; states are prepared `0`, streaming `1`, sealed `2`,
and aborted `3`. Entries are strictly ordered by `(DraftPartKind, part_key)`, which is unique within the
draft. The device returns no more than the request limit and only as many whole entries as fit the
negotiated frame. At least one entry fits the minimum. The largest response is
`44 + 6*68 = 452` payload bytes, below 496.

### 8.4 QueryWeatherRequest

The request payload is empty. The authenticated weather-read response is exactly 96 bytes:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 16 | StoreId |
| 16 | 8 | current WeatherRequestId |
| 24 | 8 | request-context revision |
| 32 | 4 | flags: head-present bit 0 |
| 36 | 8 | reserved weather LogicalObjectId |
| 44 | 8 | current weather repository Revision |
| 52 | 8 | head WeatherRequestId; inactive zero when head-present is clear |
| 60 | 4 | required centre latitude, signed degrees times 10,000,000 |
| 64 | 4 | required centre longitude, signed degrees times 10,000,000 |
| 68 | 4 | required radius metres |
| 72 | 8 | earliest issued UTC, signed Unix seconds |
| 80 | 8 | required valid-until UTC, signed Unix seconds |
| 88 | 1 | context state: pending `1`, satisfied `2` |
| 89 | 7 | reserved |

The singleton ID and repository Revision remain authoritative with no head. The request-context
revision changes whenever the desired weather context/request changes. Latitude/longitude and UTC
fields use little-endian two's-complement signed representations.

When no durable weather request context exists, an authorized query returns `objectNotFound`; it
does not synthesize a zero WeatherRequestId or an empty context. The store-reserved singleton
identity becomes visible with the first real context.

Weather StartUpload metadata carries the request ID plus the coverage/freshness facts frozen by the
domain registry; the validator requires those facts to match the payload. A current request may publish normally.
A superseded request may publish only when the sealed bundle validates against the current context,
its expected object revision still matches, and either no head exists or its validated issue time
is strictly newer than the head. It returns the distinct superseded-weather outcome and leaves the current request
pending. There is no ranking, request history, or second singleton identity.

## 9. Direct mutations

DeleteObject request is exactly 36 bytes: `OperationId[16]`, ObjectKind `u16`, flags `u16`
(expected revision bit 0 is mandatory), LogicalObjectId `u64`, and expected Revision `u64`.

SetMetadata starts with the same 36 bytes, followed by exactly one metadata envelope and then
extensions. Its minimum payload is 44 bytes. Empty patches and unknown requested fields are
rejected. Metadata changes in the same catalog commit and never through a sidecar.

InstallUpdate request is exactly 32 bytes: `OperationId[16]`, update LogicalObjectId `u64`, and
expected Revision `u64`. It requires an authenticated update-install principal and a package in
VerifiedReady state that independently passes signature, digest, target, version/downgrade, size,
power, and runtime-safety policy. CRC is irrelevant to trust. Upload never installs. No physical
confirmation is required for this explicit authenticated command.

Install crash ordering is normative: (1) durably claim OperationId; (2) validate the exact pinned
package; (3) durably commit install intent; (4) durably write and verify the boot handoff; (5)
durably commit the terminal install-requested result; (6) send and drain the response; (7) reboot.
Recovery resumes steps 3--5 idempotently and reports InProgress phase `reconciling-external-handoff`
until both intent and handoff are durable. A terminal result is never visible before the handoff.
The bootloader revalidates signature/digest and preserves trial, health confirmation, and rollback.

AcknowledgeRideImported request is exactly 32 bytes: `OperationId[16]`, ride LogicalObjectId `u64`,
and expected Revision `u64`. It is sent only after the client durably stores and verifies the
download. Download completion alone does not change import state.

Every expected revision above is checked during admission and again under the store commit lock.
Responses are ObjectResult envelopes with the operation-specific outcome.

## 10. Typed terminal results

ResultEnvelope is `result_type u8`, three reserved zero bytes, then exactly one typed body:

| Type | Value | Body size |
|---|---:|---:|
| ObjectResult | 1 | 64 |
| DraftPartResult | 2 | 88 |
| AbortResult | 3 | 56 |

ObjectResult is:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 16 | OperationId |
| 16 | 16 | StoreId |
| 32 | 2 | ObjectKind |
| 34 | 2 | outcome |
| 36 | 8 | LogicalObjectId |
| 44 | 8 | new object Revision |
| 52 | 8 | length |
| 60 | 4 | CRC |

Outcomes are committed `0`, committed-for-superseded-weather-request `1`, deleted `2`, metadata
changed `3`, update-install-requested `4`, and ride-imported `5`. Length and
CRC describe the committed/new head, or deleted old head for delete.

DraftPartResult is child OperationId `[16]`, `StoreId[16]`, parent OperationId `[16]`,
`DraftPartRef[16]`, DraftPartKind `u16`, reserved `u16`, part key `u64`, length `u64`, and CRC `u32`,
exactly 88 bytes. It has no LogicalObjectId or GenerationId.

AbortResult is abort-command `OperationId[16]`, `StoreId[16]`, target OperationId `[16]`,
disposition `u8` (cancelled `0`, already terminal `1`, already absent `2`), and seven reserved
bytes, exactly 56 bytes. `already absent` is returned only when authorization can be established
without leaking another principal's target.

Unknown result types/outcomes are preserved as bytes by diagnostic tooling but are not success to
a client that cannot name them.

## 11. OperationId claims and canonical intent

An OperationId is store-global, but status and control remain restricted to its principal scope.
After authentication, authorization, descriptor validation, and canonical intent construction, the
store atomically performs one of four actions under its claim lock:

1. no claim: proceed to resource preflight and the durable claim described below;
2. claimed by another principal: return `authorizationFailed` without comparing or exposing intent;
3. same principal and digest: return/resume the existing claim or retained terminal result;
4. same principal and different digest: return `operationIdConflict` without mutation.

For an unclaimed ID, owner/resource/space preflight and explicitly retryable domain preconditions
(such as temporary power/runtime safety) may fail without creating state. Once preflight succeeds,
the durable claim is the first mutation and precedes payload creation or any externally
visible side effect. The same atomic BeginWork/command-claim record also reserves the logical ID,
singleton slot, parent target, or draft part slot when applicable; no crash gap exists between claim
and reservation. Recovery never executes an unclaimed side effect; it resumes or durably aborts an
incomplete claimed operation. Terminal commit atomically replaces active claim state with its
result. A claim cannot be forgotten before terminal state.

AbortOperation is the only link request whose new claim uses the cancellation/recovery slot rather
than a normal claim slot; its saturation and recovery priority are frozen in Section 5.1.

For every operation-bearing mutation request, same-intent replay of retained success uses the
operation's typed successful response, while retained Aborted replay is always a `response|error`
frame for that request opcode containing exactly the stored 48-byte ErrorBody with text length
zero. It has owner none, clears retry delay/expected offset/required/available presence, and may
retain only an authoritative conflict Revision. QueryOperation is intentionally different: its
successful state `Aborted` is followed by the same bare ErrorBody so status can be inspected
without turning the query itself into a failed request.

All canonical intents begin with this exact 36-byte prefix:

| Offset | Size | Bytes |
|---:|---:|---|
| 0 | 16 | ASCII `OBC-DOS3-INTENT` plus one `00` byte |
| 16 | 16 | current StoreId |
| 32 | 2 | opcode |
| 34 | 1 | intent codec version, `1` |
| 35 | 1 | zero |

The OperationId is the lookup key and is not repeated in the digest. The principal scope is claim
ownership and is not part of semantic intent. The following exact suffixes are appended. There is
no struct padding.

| Operation | Canonical suffix, in order |
|---|---|
| StartUpload | ObjectKind `u16`; target mode `u8`; zero `u8`; LogicalObjectId `u64`; expected Revision `u64`; length `u64`; CRC `u32`; envelope length `u16`; exact canonical metadata envelope |
| BeginDraft | final ObjectKind `u16`; target mode `u8`; zero `u8`; LogicalObjectId `u64`; expected Revision `u64`; manifest length `u64`; manifest CRC `u32`; exact part count `u16`; zero `u16` |
| StartDraftPart | parent OperationId `[16]`; DraftPartKind `u16`; zero `u16`; part key `u64`; length `u64`; CRC `u32` |
| DeleteObject | ObjectKind `u16`; LogicalObjectId `u64`; expected Revision `u64` |
| SetMetadata | ObjectKind `u16`; LogicalObjectId `u64`; expected Revision `u64`; envelope length `u16`; exact canonical metadata envelope |
| AbortOperation | target OperationId `[16]`; reason `u8`; seven zero bytes |
| InstallUpdate | update ObjectKind `u16` value `7`; LogicalObjectId `u64`; expected Revision `u64` |
| AcknowledgeRideImported | ride ObjectKind `u16` value `3`; LogicalObjectId `u64`; expected Revision `u64` |

FinalizeDraft does not make a second claim and has no intent suffix. Its only request field is the
parent lookup key. Repeating it resumes the bound manifest stream or returns the retained parent
result; manifest length and CRC were already part of the BeginDraft intent.

Resume policy, RequestId, SessionId, connection, transport, chunks, nonparticipating extensions,
and human text are excluded. Inactive target fields are included as their required zero bytes, so
there is one encoding per intent. Full SHA-256 is the equality authority; CRC or a truncated digest
is forbidden.

## 12. Error body and retry matrix

ErrorBody has a 48-byte prefix and optional diagnostic text:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 2 | category |
| 2 | 2 | detail namespace: common `0` or ObjectKind |
| 4 | 2 | detail code |
| 6 | 1 | retry guidance |
| 7 | 1 | owner: none `0`, BLE `1`, USB `2`, local producer `3`, maintenance `4` |
| 8 | 2 | presence bits |
| 10 | 4 | retry-after milliseconds |
| 14 | 8 | expected offset |
| 22 | 8 | current Revision |
| 30 | 8 | required bytes |
| 38 | 8 | available bytes |
| 46 | 1 | text length, at most 64 |
| 47 | 1 | reserved |
| 48 | N | non-authoritative UTF-8 text |

Presence bits are retry delay bit 0, expected offset bit 1, current revision bit 2, required bytes
bit 3, and available bytes bit 4. Inactive values and unlisted bits are zero. Categories other than
`semanticValidation` use namespace zero. Semantic validation uses the affected ObjectKind, or
is invalid when no ObjectKind owns the semantic rule. Text is optional and never drives behavior.

| Code | Category | Permitted guidance | Required presence/detail rule |
|---:|---|---|---|
| 1 | incompatibleVersion | user action | detail unsupported major/minor |
| 2 | unsupportedCapability | never, user action | detail opcode/kind/feature |
| 3 | authenticationFailed | user action | no protected fields |
| 4 | authorizationFailed | user action | no protected fields |
| 5 | busy | retry delay, owner release | owner required; delay required for retry-delay |
| 6 | invalidFrame | never, reconnect/query | category-scoped framing detail |
| 7 | invalidDescriptor | never | category-scoped descriptor detail |
| 8 | invalidOffset | resume expected offset | expected offset required |
| 9 | invalidSession | reconnect/query | no owner token or protected state |
| 10 | objectNotFound | never, query now, refresh | no existence detail beyond authorized target |
| 11 | revisionConflict | refresh | current revision required |
| 12 | insufficientSpace | retry delay, user action | required and available bytes required |
| 13 | checksumFailure | retry same, user action | prefix mismatch uses expected offset |
| 14 | semanticValidation | never, retry same, retry delay, user action | ObjectKind namespace permitted; domain registry freezes the allowed choice |
| 15 | mediaUnavailable | retry delay, user action | delay required when retry-delay |
| 16 | mediaIo | retry delay, reconnect/query, user action | no false committed/aborted inference |
| 17 | cancelled | never | category-scoped cancellation detail |
| 18 | linkLost | reconnect/query | operation-bearing requests only |
| 19 | operationIdConflict | new ID for new intent | no prior intent/status disclosure |
| 20 | resourceLimit | retry delay, user action | required/available when meaningful |
| 21 | catalogChanged | refresh | current revision required |
| 22 | internal | retry delay, user action | stable category detail when known |

Retry guidance values are reject permanently `0`, retry same request `1`, retry after supplied delay `2`, retry
after owner release `3`, reconnect then query OperationId `4`, query OperationId now `5`, resume at
expected offset `6`, refresh catalog/domain state `7`, use a new OperationId only for genuinely new
intent `8`, and retry only after user action `9`.

Detail codes are category-scoped; the same number in another category has no relationship. Detail
zero means no narrower fact. A v3.0 sender uses only this complete table (or the semantic registry):

| Category | Namespace | Nonzero detail codes |
|---|---:|---|
| incompatibleVersion | 0 | unsupportedMajor `1`, unsupportedMinor `2` |
| unsupportedCapability | 0 | opcode `1`, logicalKind `2`, draftPartKind `3`, feature `4`, schemaVersion `5` |
| authenticationFailed | 0 | missingCredential `1`, invalidCredential `2`, expiredCredential `3` |
| authorizationFailed | 0 | principalScope `1`, operationOwner `2`, domainRead `3`, domainWrite `4`, installAuthority `5` |
| busy | 0 | heavyTransfer `1`, normalOperationClaims `2`, uploadWorkSlots `3`, draftParents `4`, draftParts `5`, readerLeases `6`, maintenanceCancellationRecoveryClaim `7`, maintenance `8`, rideSlot `9`, retainedPrevious `10` |
| invalidFrame | 0 | malformedHeader `1`, recordLength `2`, magic `3`, payloadLength `4`, frameBounds `5`, truncated `6`, trailingBytes `7` |
| invalidDescriptor | 0 | reservedBits `1`, unknownEnum `2`, invalidCombination `3`, nestedLength `4`, noncanonicalMetadata `5`, duplicateField `6`, outOfOrderField `7`, unsupportedFlags `8`, zeroRequestId `9` |
| invalidOffset | 0 | unexpectedOffset `1`, checkpointBoundary `2` |
| invalidSession | 0 | unknown `1`, staleConnection `2`, wrongPrincipal `3`, wrongLink `4`, wrongDirection `5` |
| objectNotFound | 0 | logicalObject `1`, requestedRevision `2`, draftParentUnknown `3`, operationTerminal `4` |
| revisionConflict | 0 | object `1`, repository `2`, singleton `3` |
| insufficientSpace | 0 | reservationBytes `1`, catalogCapacity `2`, retainedPrevious `3` |
| checksumFailure | 0 | wholePayload `1`, durablePrefix `2`, cursor `3` |
| semanticValidation | ObjectKind | exactly the selected registry's semantic detail table |
| mediaUnavailable | 0 | noCard `1`, unmounted `2`, recoveryReadOnly `3` |
| mediaIo | 0 | read `1`, write `2`, synchronize `3`, uncertainCommit `4` |
| cancelled | 0 | clientCancelled `1`, superseded `2`, userRequested `3`, workExpired `4` |
| linkLost | 0 | control `1`, stream `2` |
| operationIdConflict | 0 | intentDigest `1` |
| resourceLimit | 0 | minimumControlFrame `1`, minimumStreamFrame `2`, objectLength `3`, normalOperationClaims `4`, uploadWorkSlots `5`, draftParents `6`, draftParts `7`, manifestChildren `8`, readerLeases `9`, catalogHeads `10`, mountedFiles `11`, rideSlot `12` |
| catalogChanged | 0 | catalogSnapshot `1`, draftSnapshot `2`, capabilitySnapshot `3` |
| internal | 0 | invariant `1`, codec `2`, recoveryReconciliation `3` |

Category and detail must agree. Unknown received details are preserved for forward diagnostics but
do not change category retry behavior; a v3.0 implementation never invents an unregistered detail.

Validation precedence is version/framing, authentication, authorization, descriptor/schema,
idempotency claim/intent, owner/resources, compare-and-swap, size/space, then domain validation.
A later check may run early only when it cannot leak protected state; the observable error still
follows this order.

## 13. Stream frame, faults, and teardown

Every stream transport record contains one 16-byte header and payload:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | SessionId |
| 4 | 8 | absolute payload offset |
| 12 | 2 | payload length |
| 14 | 1 | direction: upload `1`, download `2`, status `3` |
| 15 | 1 | flags: fault bit 0, terminal bit 1 |

Data directions have nonempty payload, zero flags, and exact offset equal to the session's next
offset (except a negotiated download start). Status direction has offset zero. A fault status sets
fault and contains exactly 24 bytes: category `u16`, detail `u16`, expected next offset `u64`,
durable next offset `u64`, disposition `u8`, and three reserved bytes. Dispositions are resume with
new session `0`, operation durably aborted `1`, and stream transport closed/query status `2`.
Only namespace-zero transport category/details from Section 12 are valid in this compact body;
semantic/domain errors use a correlated control response. Terminal without fault is reserved and
rejected in v3.0.

For an owned, parseable stream frame with wrong offset, direction, or allowed payload size, the
receiver sends a fault status before releasing that SessionId. A resumable upload is detached at
its last durable checkpoint; a restart-only upload is durably aborted. A structurally unframeable
record, untrusted SessionId, or inability to deliver a fault closes the stream transport. The
control transport may remain available for StartUpload resume or QueryOperation. Stream errors are
never silently dropped and never reported as successful Finish.

Payload bytes beyond the last acknowledged checkpoint may be discarded. Download sources and
leases remain immutable for the session. Link teardown calls the transfer coordinator once with
the exact `(link kind, principal scope, connection generation)`; stale teardown is a no-op. It
detaches active resumable upload work and releases a matching download lease exactly once.

## 14. BLE and USB record bindings

The common frame bytes above are identical on both links. Adapters own only authentication facts,
record boundaries, pacing, timeout, and drain completion.

### 14.1 BLE

- One GATT control Write Request value contains one complete control frame. One confirmed GATT
  indication contains its complete response. Prepare/execute writes and notification-only terminal
  responses are not a v3 framing mechanism.
- One L2CAP CoC SDU contains one complete stream frame. A frame never spans SDUs and an SDU never
  contains multiple frames. The negotiated stream limit is no greater than both peers' SDU limit.
- CoC credits provide pacing only; they do not acknowledge application durability. Only a
  CheckpointUpload response advances the durable upload offset.
- Before an update reboot, the terminal InstallUpdate indication must receive its confirmation and
  previously accepted outbound records must complete, or the adapter's bounded drain timeout must
  expire. Timeout cannot undo the durable result; reconnect/boot state resolves it.

### 14.2 USB

- Control OUT and IN are independent ordered byte streams. Each record is `record_length u16`
  followed by exactly that many control-frame bytes. Length includes the 16-byte DOS header and is
  16 through the negotiated control maximum. The negotiated maximum itself must be at least 192.
- Stream OUT and IN use the same `record_length u16` prefix followed by one complete stream frame.
  Length is 17 through the negotiated stream maximum. USB packet boundaries have no protocol
  meaning; a record may span packets, but records are neither interleaved nor concatenated without
  their prefixes.
- A zero, out-of-range, prematurely terminated, or overrun record length is `invalidFrame` and
  resets only the affected USB record stream before session teardown is reported to the coordinator.
- Before update reboot, the terminal response record and all earlier IN records must complete at
  the USB device-controller/bus layer, or the bounded drain timeout must expire. Completion is
  transport drain, not proof that the host application persisted the response.

Large-object USB recommendation is a capability policy bit, not a separate operation, sink,
checksum, validator, or publication path.

## 15. Exactly-once state transitions

Upload follows `claimed -> prepared -> streaming -> sealed -> validating -> publishing ->
terminal`. Download follows `resolving -> pinned -> streaming -> completed`. Direct mutation
follows `claimed -> validating -> committing -> terminal`. Only the matching owner advances a
session. Before publishing, failure leaves the logical head unchanged. Publication and terminal
result retention are one store commit; response failure after it cannot undo success.

Draft part sealing atomically stores its opaque DraftPartRef and DraftPartResult without a logical
catalog head. FinalizeDraft atomically publishes the manifest, referenced-part reachability, and
ObjectResult. AbortOperation atomically records cancellation before releasing work. Store recovery
chooses the last durable state and never guesses from transport delivery.

The retained terminal window is bounded, not eternal exactly-once memory. Within the advertised
64-result window, same OperationId and intent deterministically resumes or returns the same result.
Outside it, safe recovery moves to domain-state reconciliation; blind replay is prohibited.
