# Device Object System v2 contract

Status: **normative** for Device Object System v2 (DOS v2). DOS v2 uses OBC control wire protocol
major **3**. The architecture generation and wire major deliberately differ because the existing
descriptor protocol already uses wire major 2.

This document is the small system-level contract: ownership, identity, state, and durability. The
implementable byte and domain contracts are split by responsibility:

- [`Device_Object_Protocol_v3.md`](Device_Object_Protocol_v3.md) — control/stream bytes, operations,
  errors, retry behavior, authorization, and BLE/USB bindings;
- [`OBC2_Storage_Format.md`](OBC2_Storage_Format.md) — private FAT layout, records, crash protocol,
  recovery, leases, and bounded resources;
- [`Device_Object_Registries_v2.md`](Device_Object_Registries_v2.md) — object/draft kinds, metadata,
  result/detail registries, and narrow domain semantics;
- [`Device_Object_Vectors_v2.md`](Device_Object_Vectors_v2.md) — independent-codec vector and
  transcript requirements.

These four documents and this index are one versioned normative suite. A change is incomplete
unless every affected document and shared vector changes together. The
[decision record](../docs/decisions/DOS1-device-object-system-v2-contract.md) explains rationale but
does not override the suite.

The previous [`obc-ble-interface-spec.md`](obc-ble-interface-spec.md) describes the legacy
descriptor protocol. It remains useful only during coordinated development. A shipping DOS v2 peer
neither translates nor serves it.

## 1. Language and representation

`MUST`, `MUST NOT`, `SHOULD`, and `MAY` have their RFC 2119 meanings. All integers are unsigned
little-endian unless explicitly signed. Reserved fields and absent fixed-width optional values are
zero and decoders reject nonzero encodings.

CRC is CRC-32/IEEE: reflected polynomial `0xEDB88320`, initial and final XOR `0xFFFFFFFF`, with
`crc32("123456789") == 0xCBF43926`. It detects accidental corruption. It is never authentication,
identity, deduplication proof, or authorization.

## 2. Ownership boundaries

The architecture has six owners with narrow semantic seams:

1. A BLE or USB adapter owns physical framing, peer identity/authentication facts, notify or drain
   ordering, and the negotiated frame limit. It implements one bounded `ByteLink` seam.
2. One board-owned `TransferCoordinator` owns the single heavy-transfer slot, SessionId, fixed
   staging buffer, current offset, and matching-owner teardown.
3. The common transfer engine owns ordered framed bytes, CRC, checkpoint/resume, and terminal
   delivery. It has one upload and one download state machine for both links.
4. A blob transaction owns one prospective immutable generation through durable claim, append,
   checkpoint, seal, typed validation, publication, or durable abort.
5. One `CardStore` owns the mounted FAT volume, catalog commit store, reader leases, recovery, and
   garbage collection.
6. Concrete borrowed repositories own authorization, semantic validation, catalog projections,
   metadata policy, and domain commands.

Transport code cannot select filenames, validators, logical revisions, or domain policy.
Repositories cannot send link frames, drain endpoints, or retain a filesystem owner. CardStore
does not grow a union of every domain method: it lends transaction, lease, and catalog capabilities
to one concrete repository at a time.

Storage-private `GenerationId` and paths do not cross the public repository/client seam. Multipart
repositories receive an authenticated opaque parent-bound `DraftPartRef`; it conveys no authority,
and only CardStore resolves it to a sealed generation while validating and committing the matching
manifest.

## 3. Mechanically distinct identities

Every first-party language uses distinct types with no implicit cross-conversion.

| Type | Representation | Scope and rule |
|---|---:|---|
| StoreId | 16 opaque bytes | Born with the first valid OBC2 checkpoint. Reformat/card replacement creates a new value. |
| LogicalObjectId | `u64` | Opaque inside one StoreId and ObjectKind. No sentinel or client-reserved band. |
| Revision | `u64` | Monotonic repository/object compare-and-swap token. Zero is a value, never absence. |
| GenerationId | `u64` | Store-private immutable payload identity. Never a companion link. |
| OperationId | 16 opaque bytes | Globally unique client/local-producer mutation identity, chosen before intent claim. |
| SessionId | nonzero `u32` | Ephemeral capability for one stream and one authenticated connection owner. |
| RequestId | nonzero `u32` | Control request/response correlation only. |
| WeatherRequestId | `u64` | Durable weather-domain request identity, never a control RequestId. |
| DraftPartRef | 16 opaque bytes | Device-issued authenticated reference scoped to one durable multipart parent; never authorization. |

Sixteen-byte identities are copied verbatim and have no integer/UUID field byte order. Their
diagnostic spelling is 32 lower-case hexadecimal digits in wire-byte order. UUID field reordering
is forbidden.

A companion persistence key is `(device serial, StoreId, ObjectKind, LogicalObjectId)`. Revision is
its concurrency token. Filenames, display names, CRC, length, OperationId, DraftPartRef, and
GenerationId are not logical identity.

OperationIds are never reused by a conforming producer, including after their result is evicted.
Changing StoreId changes intent identity, so work from a removed card cannot attach to its
replacement.

## 4. Repository revisions and immutable publication

Each repository has one monotonic Revision. A logical mutation increments it exactly once; an
entry's Revision is the repository revision of that entry's latest mutation. The same ordering
drives compare-and-swap, snapshot catalog paging, and coalescing CommitEvent catch-up.
Revision never wraps; a repository at `u64::MAX` rejects further mutation as `resourceLimit` until
an explicit StoreId-changing reset.

Each repository retains its most recently displaced immutable generation as the one historical
revision available to a requested-revision download. Live leases and update rollback may retain
additional generations without changing which one is the repository previous revision. Capacity
preflight rejects a mutation before publication when those temporary pins exhaust the bounded
retention table.

Transferred or locally produced bytes always create a prospective physical generation. Existing
generations are never overwritten. Publication occurs only after bytes are sealed and synchronized,
length/CRC checks pass, the concrete typed validator succeeds, and the expected Revision is
rechecked under the CardStore commit lock.

One durable catalog record contains the logical/repository transition, new Revision, canonical
OperationId intent digest, and terminal result. Its final validity gate is the only publication
point. Before that gate recovery exposes the old state; after it recovery exposes the new state and
the queryable result. Response loss changes delivery, not truth.

Metadata update, delete, ride acknowledgement, and update-install request use the same commit
discipline. Multipart child sealing is durable draft work, not publication; only the parent's
manifest commit makes its sealed children reachable as a release.

## 5. Operation identity and bounded exactly-once scope

After authentication, authorization, complete descriptor/schema validation, and preflight checks
that intentionally do not claim an operation, storage durably records `(StoreId, principal,
OperationId, full SHA-256 canonical intent)` before accepting mutation bytes or reporting a durable
reservation. That record is the claim point. The wire contract freezes the canonical bytes and
which rejection classes occur before or after it.

The same OperationId and digest resumes or returns its work/result. The same OperationId with a
different digest is `operationIdConflict` and changes nothing. Session, RequestId, connection,
transport, chunking, and resume preference never participate in logical intent.

The store retains exactly the latest 64 terminal records, committed or durably Aborted, ordered by
terminal catalog sequence. Active work is separately bounded and does not consume those slots.
Within that advertised window a lost result is recoverable through `QueryOperation`, including
after disconnect/reboot. The 64th newer terminal record deterministically evicts the oldest.

This is a bounded exactly-once guarantee, not an unbounded tombstone service. After eviction,
`Unknown` cannot distinguish never-seen from previously terminal. First-party clients persist
OperationId before sending, query uncertain jobs promptly, never reuse it, and reconcile catalog or
domain state after an evicted uncertainty; they never blindly replay it. Devices expose capacity
64, and client tests prove retention after 63 newer terminals and eviction by the 64th. No
wall-clock retry promise or hidden operation archive exists.

## 6. Session ownership and concurrency

A SessionId is valid only with its adapter link kind, authenticated principal, and connection
generation. Reconnect creates a new generation even for the same principal. A SessionId from
another wire or older connection cannot append, finish, abort, release a lease, or tear down the
current owner.

Only the coordinator may issue/revoke sessions. It never reuses a numeric SessionId within one
connection generation; it reconnects before exhausting the nonzero space. Wrong-owner data or
teardown leaves the current session and storage capability unchanged. Link loss releases ephemeral ownership, not durable
resumable work. A later authenticated same-principal request obtains a fresh SessionId at the last
durable offset. Durable abandonment addresses OperationId rather than a stale session.

Reader pin acquisition linearizes catalog resolution and lease allocation under CardStore.
Replacement or delete does not retarget/revoke an acquired lease. Lease tokens include slot
generation so a stale close cannot decrement a reused slot.

## 7. Explicit state boundaries

An upload follows:

```text
Unclaimed -> IntentClaimed -> Prepared -> Streaming -> Sealed
          -> Validated -> Publishing -> ResultRetained
```

Failure before intent claim has no durable operation identity. Failure after claim is either a
retryable InProgress state or a durable Aborted terminal record. During Publishing the one catalog
gate selects old versus new. No state reports success before ResultRetained.

A download follows:

```text
Resolving -> Pinned -> Streaming -> Completed -> Released
```

The pinned source never changes. Matching completion or abort releases it once. A malformed finish
or stale-owner teardown cannot release another reader's generation.

A command follows:

```text
Received -> Authenticated -> Authorized -> IntentClaimed
         -> DomainValidated -> Committing -> ResultRetained
```

Revision and safety facts are rechecked immediately before the commit. An implementation may do
expensive validation outside the owner lock, but cannot publish from a stale admission-time check.

## 8. Recovery, resource, and transport invariants

Recovery is incremental and idempotent. Until the catalog and active-work projection are known,
queries return recovery-in-progress rather than premature Unknown. Corruption never triggers
filename reconstruction, silent reformat, or speculative deletion. Garbage collection deletes
only a known-format generation proven unreachable from heads, retained predecessors, transitive
manifest children, update trial/rollback state, active/sealed work, and live leases.

All queues, work slots, draft parts, catalog projections, results, leases, frames, and metadata are
bounded by compiled and advertised capacities. Oversize or over-capacity work is refused before
payload transfer or partial allocation. No operation allocates an unbounded queue, creates a task,
or requires a whole large payload in RAM.

BLE and USB share every semantic policy: identities, operations, offsets, CRC, validators,
checkpointing, publication, results, and owner checks. Physical authentication, frame carriage,
credit/endpoint pacing, timeout, and terminal drain are adapter facts frozen in the wire binding.
Transport preference is a capability/cost hint, never a different object kind or storage path.

## 9. Update safety

Uploading an update publishes only VerifiedReady after independent signature, digest, target,
version/downgrade, and size validation. CRC is not trust. A separate authenticated and authorized
InstallUpdate command may automatically arm and reboot without physical confirmation after power
and runtime safety checks.

The OBC2 format defines the crash-safe catalog-to-boot-handoff protocol. The bootloader independently
revalidates the package and enforces trial boot, application health confirmation, and rollback.
Merely completing transport can never install an update.

## 10. Coordinated cutover

DOS v2 is one breaking cutover across firmware, on-card storage, iOS, TypeScript clients, desktop,
simulator, and test devices. Existing descriptor versions, 16-bit identities, sentinel bands,
filename identity/reconciliation, temporary upload names, held-magic promotion, private domain
runners, and old card objects are not translated or imported.

A legacy peer receives an incompatibility result only where the new header can be parsed; otherwise
discovery fails closed. An old card initializes a new OBC2 StoreId. Legacy directories remain
non-authoritative until explicit cleanup. Runtime dual read, dual write, compatibility forwarding,
and best-effort translation are forbidden.
