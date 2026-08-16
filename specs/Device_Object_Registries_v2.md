# Device Object System v2 registries

Status: **normative** for Device Object System v2. The
[system contract](Device_Object_System_v2.md) owns identities and layer boundaries; the
[wire contract](Device_Object_Protocol_v3.md) owns message layouts. This registry owns stable
numeric assignments and bounded domain schemas. A domain changes only its own section and the
shared vectors; it does not broaden a common repository or transport interface.

All integers are unsigned little-endian unless marked signed. Metadata uses the envelope and field
codec defined by the wire contract. Unknown fields in mutating requests are rejected. Catalog
readers may skip unknown noncritical fields and reject unknown critical fields.

## 1. Logical object kinds

`ObjectKind` is `u16`.

| Value | Kind | Logical lifecycle |
|---:|---|---|
| 0 | invalid | Never encoded. |
| 1 | route | Create, compare-and-swap replace, metadata update, list, download, delete. |
| 2 | trip | Create, compare-and-swap replace, list, download, delete. |
| 3 | ride | Exactly-once finalization, list, download, explicit import acknowledgement, delete. |
| 4 | weather | One store-owned singleton identity, replace, list, download, delete. |
| 5 | reserved | Must not be advertised or encoded. Draft parts are not logical objects. |
| 6 | volume manifest | Create/replace one atomic release head, metadata update, list, download, delete. |
| 7 | update package | Publish VerifiedReady, list, download, explicit install, retention/rollback cleanup. |

Trip remains optional but distinct because it has an independent name, ordered route membership,
and replace/list/download/delete lifecycle. `DeleteObject(Trip)` removes only the
trip. Deleting its routes is a client-composed sequence, not an implicit multi-object transaction.

A route head is the validated canonical route payload. Original GPX/TCX source bytes are not a
sidecar, alternate identity, or implicitly retained generation. A future named lossless-source
export feature must define a separate logical lifecycle and contract before storing them.

## 2. Multipart draft registry

Draft parts are immutable prospective generations, not logical objects. They have no
`LogicalObjectId`, repository `Revision`, catalog entry, or public physical identifier.

`DraftPartKind` is `u16`:

| Value | Kind |
|---:|---|
| 0 | invalid |
| 1 | standalone map blob |
| 2 | map shard |
| 3 | terrain blob |
| 4 | volume index |

A `DraftPartRef` is a 16-byte, device-issued authenticated opaque reference meaningful only inside
the parent draft that issued it. It conveys no authority: authentication and authorization are
checked independently. It is not a `GenerationId`, content digest, filename, or globally
addressable object ID. CardStore's private keyed codec makes it resolvable after publication
without exposing its physical identity. Clients may place it only in the matching volume-manifest
payload.

### 2.1 Parent lifecycle

`BeginDraft` durably claims a parent `OperationId` and binds all of these facts before a child can
claim storage:

- target kind, which is initially only `volume manifest`;
- create versus replace, target LogicalObjectId when replacing, and expected Revision;
- declared final manifest length and CRC;
- exact expected part count, from 1 through the advertised maximum;
- the authenticated principal and StoreId used for authorization and intent identity.

The parent has a monotonic `DraftRevision` beginning at 1. Claiming, sealing, or durably aborting a
child increments it. `(DraftPartKind, part_key u64)` is unique within one parent; a duplicate with
the same child OperationId and intent resumes, while any different intent is
`operationIdConflict` or semantic `duplicateDraftPart` before allocation.

`StartDraftPart` durably claims a child OperationId, parent OperationId, part kind/key, and declared
length/CRC. `DraftPartAccepted` returns only its stream session and durable offset; the ref does not
exist yet. Successful FinishUpload seals the part, mints the opaque DraftPartRef, and returns it in
`DraftPartResult`; it never returns a logical result. Finalization uploads or validates the bound
manifest bytes under the **parent** OperationId, proves that it references exactly the sealed
DraftPartRefs and declared count, rechecks the target Revision under the CardStore commit lock, and
publishes one volume-manifest logical head. That one commit makes the manifest and all referenced
parts reachable.

An expired or explicitly abandoned parent first durably enters `aborting`, which prevents new
children or finalization. Recovery terminally aborts each nonterminal child, then the target parent,
and only then commits the separate abort-command result. Expiry is pressure-based rather than
wall-clock-based: storage may reclaim a draft only through the advertised reclamation policy and
this same ordered sequence. An OperationId or DraftPartRef from an aborted, finalized, or evicted
parent can never be rebound.

The baseline contract supports at most 32 parts per draft. A device may advertise a smaller value
but MUST reject the parent during `BeginDraft`, before child bytes transfer, when the declaration
exceeds it. Pagination is snapshot-bound to DraftRevision and rejects a changed revision instead of
mixing child sets.

### 2.2 Volume-manifest payload v1

The final logical payload is a bounded binary manifest, not an on-card filename table. Its
96-byte header is:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | magic ASCII `OBCV` |
| 4 | 2 | version `1` |
| 6 | 2 | header length `96` |
| 8 | 2 | entry count, 1 through 32 and equal to BeginDraft's declared part count |
| 10 | 2 | flags, zero |
| 12 | 4 | map schema revision |
| 16 | 4 | south latitude, signed microdegrees |
| 20 | 4 | west longitude, signed microdegrees |
| 24 | 4 | north latitude, signed microdegrees |
| 28 | 4 | east longitude, signed microdegrees |
| 32 | 1 | UTF-8 display-name length, 1 through 32 |
| 33 | 32 | display-name bytes followed by zero |
| 65 | 16 | parent OperationId from BeginDraft |
| 81 | 15 | zero |

Exactly `entry_count` 56-byte records follow immediately:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 16 | parent-scoped DraftPartRef |
| 16 | 2 | DraftPartKind |
| 18 | 2 | flags: core map coverage bit 0; other bits zero |
| 20 | 8 | part key |
| 28 | 8 | exact child length |
| 36 | 4 | exact child CRC-32 |
| 40 | 4 | child south latitude, signed microdegrees |
| 44 | 4 | child west longitude, signed microdegrees |
| 48 | 4 | child north latitude, signed microdegrees |
| 52 | 4 | child east longitude, signed microdegrees |

The payload length is exactly `96 + entry_count * 56`. The header parent must equal the parent
OperationId used for finalization and DraftPartRef decoding. Records are strictly ordered by
`(DraftPartKind, part_key)` and unique. Every bbox is ordered and inside the manifest bbox. Exactly
one map-bearing entry has the core bit and its bbox equals the manifest bbox; terrain/index entries
never set it. A standalone-map entry requires entry count one. The validator resolves every ref
under this parent and requires kind, key, length, and CRC equality before publication. Display
name, part count, and selected state in catalog metadata are validator-derived; selected state is
the separate metadata flag and is not hidden in this payload. Initial publication derives selected
as false; selecting the release requires a later compare-and-swap SetMetadata operation.

## 3. Weather singleton and durable request context

Store initialization reserves exactly one weather LogicalObjectId and persists it even when no
weather head exists. It is an ordinary `u64` value, not a sentinel. The device exposes it in each
durable weather-request context; an authorized query before any context exists returns
`objectNotFound`. Clients never choose it. Deleting weather removes
only the head; the reserved identity and repository revision survive, and later replacement uses
that same identity plus the current Revision.

Deleting the weather head atomically clears the durable head-present flag and inactive head request
ID, advances the weather repository Revision, and returns the existing request context to pending.
It preserves WeatherRequestId, request-context revision, singleton identity, and requested
coverage/time facts so the connection-independent service can satisfy the same request again.

The weather domain owns one durable, connection-independent request context:

| Field | Type | Rule |
|---|---:|---|
| WeatherRequestId | `u64` | Monotonic domain ID; never a control RequestId. |
| weather LogicalObjectId | `u64` | Store-owned singleton identity. |
| repository Revision | `u64` | CAS token to use for the response. |
| centre latitude | `i32` | Degrees times 10,000,000, range -900,000,000 through 900,000,000. |
| centre longitude | `i32` | Degrees times 10,000,000, range -1,800,000,000 through 1,800,000,000. |
| required radius metres | `u32` | Nonzero and at most 100,000. |
| earliest issued UTC | `i64` | Trusted Unix seconds. |
| required valid-until UTC | `i64` | Must be later than earliest issued UTC. |
| context state | `u8` | pending `1`, satisfied `2`; zero and other values invalid. |

Creating or replacing this request is a local durable domain transition and increments both
WeatherRequestId and a separate request-context revision. It does **not** increment the weather
object repository Revision or mutate the catalog head; only publishing/deleting the logical
weather object does that. This separation lets a response become superseded while its original
object CAS token can still be valid. A client discovers the complete context through
`QueryWeatherRequest`; no connection event or unsolicited frame is authoritative. If trusted time
or position is unavailable, the domain does not create a request it cannot validate.
Neither counter wraps; exhaustion requires explicit StoreId-changing reset rather than reuse.

### 3.1 Weather Put metadata v1

Every field is critical and required:

| Tag | Type | Meaning |
|---:|---:|---|
| `0x8001` | `u64` | WeatherRequestId answered by this bundle. |
| `0x8002` | `i32` | Validated coverage centre latitude. |
| `0x8003` | `i32` | Validated coverage centre longitude. |
| `0x8004` | `u32` | Validated coverage radius metres. |
| `0x8005` | `i64` | Validated issued UTC. |
| `0x8006` | `i64` | Validated valid-until UTC. |

These are declared semantic facts used for bounded preflight. The typed weather validator MUST
derive the same facts from the payload; any mismatch is `weather.payloadFactsMismatch`. The payload
remains the weather data authority, while the catalog stores the validator-derived facts.

The superseded-request rule is deliberately narrow. If the request ID is current and normal
coverage/time validation passes, publication satisfies it. If it has been superseded, publication
is allowed only when the expected repository Revision still matches, the bundle passes the current
request's coverage and validity predicates, and either no head exists or its issued UTC is strictly
later than the current head. The result is `committedSupersededWeather` and the newer request
remains pending. Otherwise
the mutation aborts as `weather.supersededNotUseful`. No history, provider ranking, or quality
score participates.

## 4. Metadata envelopes

Put schemas use version `1`, patch schemas version `128`, and catalog projection schemas version
`64`. An empty schema is still an eight-byte envelope with zero fields.

The envelope `schema_id` is the numeric ObjectKind. The wire field codec fixes the four-byte field
header, canonical ordering, integer and text encodings, and rejection rules. These are the exact
maximum encoded envelope lengths for the registered schemas; a change to any field or bound must
update this table and the shared maximum vectors.

| Kind | Put v1 | SetMetadata v128 | Catalog v64 |
|---|---:|---:|---:|
| route | 13 | 70 | 82 |
| trip | 8 | unsupported | 66 |
| ride | 8 | unsupported | 41 |
| weather | 68 | unsupported | 44 |
| volume manifest | 8 | 13 | 55 |
| update package | 8 | unsupported | 77 |

Every length includes the eight-byte envelope. A decoder rejects a schema-specific envelope larger
than its value above even though the common Put/patch and catalog envelope ceilings are 128 and 96
bytes respectively.

### 4.1 Put v1

| Kind | Tag | Type | Meaning |
|---|---:|---|---|
| route | `0x8001` | `u8` | Retention: never `0`, day `1`, week `2`, two weeks `3`, month `4`, two months `5`. |
| weather | §3.1 | — | All six weather response facts. |

Trip, ride, volume-manifest, and update-package Put v1 have zero fields. Their semantic facts are
derived from validated payload bytes. Draft parts use their dedicated command, not this schema.

### 4.2 SetMetadata v128

Every present field is applied in one catalog commit. An empty patch is invalid.

| Kind | Tag | Type | Meaning |
|---|---:|---|---|
| route | `0x8001` | `u8` | Retention, using the Put values. |
| route | `0x8002` | `u8` | Selected boolean, exactly 0 or 1. |
| route | `0x8003` | UTF-8, 1–48 bytes | Display name. |
| volume manifest | `0x8001` | `u8` | Selected boolean, exactly 0 or 1. |

Other kinds reject SetMetadata as unsupported. Ride import state changes only through its explicit
command; trip name and stages change through payload replacement.

### 4.3 Catalog projection v64

Catalog projection envelopes are at most 96 bytes and contain validator-derived bounded facts.

| Kind | Tag | Type | Meaning |
|---|---:|---|---|
| route | `0x8001` | UTF-8, 1–48 bytes | Display name. |
| route | `0x8002` | `u8` | Retention. |
| route | `0x0003` | `u8` | Selected, optional. |
| route | `0x0004` | `i64` | Trusted creation UTC, optional. |
| trip | `0x8001` | UTF-8, 1–48 bytes | Display name. |
| trip | `0x8002` | `u16` | Ordered stage count. |
| ride | `0x8001` | `i64` | Start UTC. |
| ride | `0x8002` | `u32` | Duration seconds. |
| ride | `0x8003` | `u32` | Distance metres. |
| ride | `0x8004` | `u8` | Imported acknowledgement boolean. |
| weather | `0x8001` | `u64` | WeatherRequestId that produced the head. |
| weather | `0x8002` | `i64` | Issued UTC. |
| weather | `0x8003` | `i64` | Valid-until UTC. |
| volume manifest | `0x8001` | UTF-8, 1–32 bytes | Display name. |
| volume manifest | `0x8002` | `u8` | Selected boolean. |
| volume manifest | `0x8003` | `u16` | Referenced part count. |
| update package | `0x8001` | UTF-8, 1–24 bytes | Validated semantic version. |
| update package | `0x8002` | `u8` | State. |
| update package | `0x8003` | 32 bytes | Validated image digest. |

Update states are VerifiedReady `1`, installRequested `2`, trial `3`, confirmed `4`, rolledBack
`5`, and failed `6`.

## 5. Result outcomes

Logical `OperationResult` outcomes are committed `0`, committedSupersededWeather `1`, deleted `2`,
metadataChanged `3`, updateInstallRequested `4`, and rideImported `5`. `DraftPartResult` is a
different message type and has no logical outcome, ID, or Revision fields.

Storage-local `DomainResult` outcomes are weatherRequestChanged `1` and updateStateChanged `2`.
They use a device-local authenticated principal and remain subject to the same claim/result atomicity
and latest-64 retention. Link principals cannot query them.

## 6. Semantic detail registry

The error body's detail namespace is its ObjectKind, or common namespace `0`. A server returns only
details listed here; clients may display but do not parse diagnostic text.

Common framing, ownership, snapshot, and retry details are owned by the wire contract. This file
allocates only ObjectKind-scoped semantic-validation details.

| Namespace | Code | Detail | Terminal? |
|---|---:|---|---|
| route | 1 | invalidRouteFormat | yes |
| route | 2 | missingTripRoute | yes |
| trip | 1 | invalidTripFormat | yes |
| trip | 2 | duplicateRouteReference | yes |
| ride | 1 | invalidRideFormat | yes |
| ride | 2 | alreadyImported | no; same semantic state returns retained success when same operation |
| weather | 1 | supersededNotUseful | yes |
| weather | 2 | coverageMismatch | yes |
| weather | 3 | staleBundle | yes |
| weather | 4 | payloadFactsMismatch | yes |
| weather | 5 | requestMismatch | yes |
| volume manifest | 1 | invalidManifest | yes |
| volume manifest | 2 | missingDraftPart | yes |
| volume manifest | 3 | foreignDraftPart | yes |
| volume manifest | 4 | duplicateDraftReference | yes |
| volume manifest | 5 | duplicateDraftPart | yes |
| volume manifest | 6 | draftNotOpen | yes |
| volume manifest | 7 | draftIncomplete | no; retry same after remaining parts seal |
| update package | 1 | invalidSignature | yes |
| update package | 2 | digestMismatch | yes |
| update package | 3 | wrongTarget | yes |
| update package | 4 | downgradeDenied | yes |
| update package | 5 | packageTooLarge | yes |
| update package | 6 | unsafePowerState | no |
| update package | 7 | unsafeRuntimeState | no |
| update package | 8 | notVerifiedReady | yes |

Format, signature, target, digest, downgrade, request mismatch, and semantic validation failures are
terminal Aborted after the OperationId has been durably claimed. Transient power/runtime, media,
owner, and recovery conditions do not terminally claim a new command; their required retry
guidance is frozen in the wire contract.

Unless a row says nonterminal, it uses category `semanticValidation` with reject-permanently
guidance. `draftIncomplete` uses retry-same-request. `unsafePowerState` and `unsafeRuntimeState`
use retry-after-delay when the device can supply a bounded delay, otherwise retry-after-user-action.
`alreadyImported` is not emitted for the same retained acknowledgement intent; that replay returns
its committed result. A different acknowledgement intent observes normal revision conflict.

## 7. Update and ride command semantics

An update upload publishes only VerifiedReady after signature, digest, target, version/downgrade,
and size validation. A separate authenticated and authorized `InstallUpdate` may automatically arm
and reboot without physical confirmation once power/runtime safety also passes. Transfer CRC is
never trust. Bootloader revalidation, trial boot, application health confirmation, and rollback
remain mandatory.

Every observed post-reboot state—trial, confirmed, rolledBack, or failed—is applied by a fresh
device-local update operation. Its one terminal commit updates the package head's catalog metadata,
increments the update repository Revision, advances the handoff projection, emits the coalescing
CommitEvent, and stores DomainResult. Repeating recovery after a cut resumes the same local
OperationId; it never invents a second state transition.

`AcknowledgeRideImported` is accepted only for the current ride Revision after the caller durably
stores and verifies the download. Download completion alone never changes import state. Repeating
the same command and intent returns the retained result.

An active ride uses one device-local OperationId from start through immutable finalization. Its
optional route association is a historical `(LogicalObjectId, Revision)` snapshot captured at ride
start; later route replacement or deletion neither retargets nor invalidates the ride. Recovery
exposes a typed recoverable active journal to the ride domain. Whether the UI resumes, finalizes, or
discards it is a DOS6 product policy and does not change its storage identity or exactly-once
publication seam.
