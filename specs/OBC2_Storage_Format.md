# OBC2 storage format

- Status: normative for Device Object System v2
- Format version: 1
- Wire contract: [`Device_Object_Protocol_v3.md`](Device_Object_Protocol_v3.md)
- Replaces: every earlier device-object catalog, staging, promotion, and sidecar mechanism

This document specifies the private, crash-safe FAT representation owned by `CardStore`. It does
not specify domain validation, wire framing, or a filesystem-shaped public API. Integers are
little-endian. Byte offsets are zero-based. Reserved bytes are written as zero and must be zero
when read. A decoder rejects arithmetic overflow, duplicate keys, out-of-order entries, an unknown
nonzero tag, or a count above its stated capacity before using any derived offset.

There is no DOS v1 import, discovery, dual-write, or fallback path. An explicit format/reset is the
only transition from another card layout and creates a new `StoreId`.

## 1. Ownership and durability vocabulary

`CardStore` is the only component allowed to mutate `/OBC2`. Its policy layer owns operation
claims, catalog compare-and-swap, revisions, draft membership, result retention, reachability, and
garbage collection. A FAT adapter owns paths, file handles, allocation, reads, writes, and
`sync_media`. Domain repositories provide validated projections and immutable payloads; they never
open these files directly.

`sync_media` means that all earlier filesystem changes, block-cache writes, card commands, and the
card's final not-busy state have completed successfully. An adapter that can only flush a software
buffer does not implement this contract. A failed sync has an uncertain outcome and is resolved by
recovery; it is never evidence that the preceding mutation did not commit.

Every gated record has a body and a physically disjoint 512-byte gate sector. Writers invalidate
and synchronize an old gate before reusing its body, synchronize the complete new body, then write
and synchronize the new gate. Readers require both CRCs and all structural invariants. This detects
the specified torn-write cases with CRC-32's residual collision probability; CRC is not an
authenticity mechanism.

CRC fields use CRC-32/IEEE: reflected polynomial `0xEDB88320`, initial value and xor-out
`0xFFFF_FFFF`, and check value `CRC32("123456789") = 0xCBF43926`. A CRC field is treated as zero
while its containing record is checksummed.

## 2. Contract capacities

These are v1 format and product limits, not values inferred from available RAM at runtime. The
wire ResourceLimits block reports them, while per-subject capabilities report maximum lengths;
admission enforces every limit before creating payload bytes. Raising a limit requires a resource
review; changing a fixed array size or file size is an OBC2 format-version change.

| Limit | Value |
| :-- | --: |
| Logical catalog heads, all kinds | 256 |
| Route heads | 64 |
| Trip heads | 16 |
| Ride heads | 128 |
| Weather heads | 1 |
| Volume-manifest heads | 8 |
| Update-package heads | 8 |
| Normal active claimed operations | 8 |
| Reserved maintenance/cancellation/recovery claims | 1 |
| Active resumable upload/work records | 4 |
| Attached heavy stream sessions, system-wide | 1 |
| Active draft parents | 2 |
| Sealed or streaming draft parts, all parents | 32 |
| Children referenced by one manifest | 32 |
| Simultaneously mounted map data files on the current board | 11 |
| Live download leases | 4 |
| Retained previous generations | 16 |
| Active or recoverable ride journals | 1 |
| Terminal operation results | 64 |
| Journal slots | 256 |
| Journal compaction trigger | 192 valid slots |
| Inactive-work retention horizon | 256 later terminal commits |
| Maximum one embedded FAT generation | `0xFFFF_FFFF` bytes |

Map draft parts do not consume logical-head slots. A selected volume manifest may reference at most
11 files that must be held open together on the current board; an unselected manifest may contain
up to 32 children. Selection is rejected before publication if its mount set exceeds the board
limit. Per-kind limits and the all-kind limit are both enforced.

There is deliberately no wall-clock work TTL. The device cannot assume trusted time, so resumable
work expires after 256 terminal commits following its last durable progress. Expiry is itself a
terminal `Aborted` commit before files become collectable. If no commits occur, work remains
resumable; the bounded active-work capacity prevents unbounded growth.

The 64-result guarantee is likewise count-bound. A terminal result remains queryable while it is
one of the latest 64; the 64th newer terminal commit evicts it. After eviction, `QueryOperation`
returns `Unknown`, which is an indeterminate old outcome, not permission to retry that identity. A
client must never reuse an `OperationId` for the lifetime of its installation, even after card
reset or a result becomes unknown. OBC2 stores no unbounded tombstone set to compensate for a
client violating that rule.

## 3. FAT tree and identity mapping

Firmware creates uppercase FAT 8.3 names only. It does not require rename.

```text
/OBC2/
  CAT0.CHK       checkpoint A, 74,240 bytes
  CAT1.CHK       checkpoint B, 74,240 bytes
  COMMIT.JNL     256 fixed slots, 524,288 bytes
  ARM0.HND       update-handoff A, 1,024 bytes
  ARM1.HND       update-handoff B, 1,024 bytes
  RIDE.ACT       active-ride recovery journal, 16,384 bytes
  INIT.REC       incomplete-initialization witness, 1,024 bytes
  GEN/
    XX/
      BBBBBBBB.BBB
  WORK/
    XX/
      BBBBBBBB.BBB
```

`XX` is the low byte of `GenerationId` as two uppercase hexadecimal digits. The 11 `B` characters
are `GenerationId >> 8` encoded as fixed-width uppercase base-36 and split into an eight-character
stem and three-character extension. Because `36^11 > 2^56`, the mapping is reversible and
collision-free. The same leaf identifies a raw payload under `GEN` and its record under `WORK`.
Opening a leaf consumes the four configured directory handles: volume root, `OBC2`, role, shard.

`GEN` files are exactly the canonical payload bytes and contain no OBC2 wrapper. A generation is
store-global, monotonically reserved, never reused, and never wrapped. Zero is a valid first
GenerationId; record state/presence, never its numeric value, distinguishes absence. Generation
filenames are private and never serve as logical identities or wire references.

## 4. Common gate sector

The final 512 bytes of every gated slot have this layout:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | gate magic, specific to the containing record |
| 4 | 2 | format version, `1` |
| 6 | 2 | physical slot index |
| 8 | 8 | scope: epoch, `GenerationId`, handoff sequence, or initialization binding |
| 16 | 8 | logical sequence represented by the body |
| 24 | 4 | body CRC-32 |
| 28 | 4 | one's complement of body CRC-32 |
| 32 | 4 | gate CRC-32 over all 512 gate bytes with this field zero |
| 36 | 476 | zero |

A gate is valid only when its magic and slot index match its physical location, its version is
known, the complement is exact, its body CRC equals both the body's stored CRC and a fresh CRC of
the body, its scope and sequence equal the body, and its gate CRC validates. Gate magics are
`O2CG` (checkpoint), `O2JG` (journal), `O2WG` (work), `O2RG` (ride recovery), `O2HG`
(handoff), and `O2IG` (initialization).

## 5. Catalog checkpoint

Each checkpoint is 73,728 body bytes followed by one gate sector, for 74,240 bytes total. The body
CRC is at bytes `73724..73728`. Its gate uses physical slot 0 or 1, scope `epoch`, and logical
sequence `through_sequence`.

### 5.1 Fixed regions

| Byte range | Entry shape | Capacity |
| :-- | :-- | --: |
| `0..128` | checkpoint header | 1 |
| `128..512` | repository state, 24 bytes | 16 |
| `512..49664` | catalog head, 192 bytes | 256 |
| `49664..50816` | active operation, 128 bytes | 9 |
| `50816..51072` | draft parent, 128 bytes | 2 |
| `51072..54144` | draft part, 96 bytes | 32 |
| `54144..55168` | retained previous generation, 64 bytes | 16 |
| `55168..68480` | terminal result, 208 bytes | 64 |
| `68480..68720` | update handoff projection, 240 bytes | 1 |
| `68720..68800` | weather request state, 80 bytes | 1 |
| `68800..68928` | active ride state, 128 bytes | 1 |
| `68928..73724` | zero | — |
| `73724..73728` | body CRC-32 | 1 |

Entries in each occupied prefix are sorted by their stated key and the remaining entries are all
zero. Counts in the header select the occupied prefix. The result region is the sole exception: it
is a circular array described by `result_start` and `result_count`.

### 5.2 Header

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `O2CK` |
| 4 | 2 | format version `1` |
| 6 | 2 | header length `128` |
| 8 | 16 | `StoreId` |
| 24 | 8 | epoch, nonzero |
| 32 | 8 | through-sequence |
| 40 | 8 | next `GenerationId` |
| 48 | 2 | repository-state count |
| 50 | 2 | catalog-head count |
| 52 | 1 | active-operation count, `0..9`; at most eight normal rows and at most one reserved-flag row |
| 53 | 1 | draft-parent count |
| 54 | 1 | draft-part count |
| 55 | 1 | retained-previous count |
| 56 | 1 | result start index, `0..63` |
| 57 | 1 | result count, `0..64` |
| 58 | 1 | handoff count, `0..1` |
| 59 | 1 | flags; bit 0 is recovery-degraded, all others zero |
| 60 | 8 | terminal-commit counter used for work expiry |
| 68 | 4 | fixed body bytes, `73728` |
| 72 | 32 | private DraftPartRef key |
| 104 | 1 | weather-state count, `0..1` |
| 105 | 1 | active-ride-state count, `0..1` |
| 106 | 22 | zero |

The header counts must agree with the decoded regions. The active region is valid only with at most
eight rows whose reserved-slot flag is clear, at most one whose flag is set, and at most nine total;
a completed reserved operation removes its row rather than retaining a terminal row here. `next
GenerationId` is greater than every reserved generation. Sequences and generation IDs never wrap;
reaching `u64::MAX` mounts the store read-only until explicit reset.

### 5.3 Projection entries

Repository states are keyed by `ObjectKind`: kind `u16`, flags `u16` (logical-ID space exhausted
bit 0), four zero bytes, revision `u64`, and next logical-ID candidate `u64`. Zero is a valid first
candidate. Allocating `u64::MAX` sets exhausted rather than wrapping; failed/aborted creates do not
put their reserved ID back.

A catalog head is keyed by `(ObjectKind, LogicalObjectId)`:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 1 | occupied, exactly `1` |
| 1 | 1 | zero |
| 2 | 2 | object kind |
| 4 | 8 | logical object ID |
| 12 | 8 | repository revision of this head |
| 20 | 8 | generation ID |
| 28 | 8 | payload length |
| 36 | 4 | payload CRC-32 |
| 40 | 2 | catalog-projection envelope length, `8..96` |
| 42 | 6 | zero |
| 48 | 128 | canonical catalog-projection envelope followed by zero |
| 176 | 16 | zero |

An active operation is keyed by `OperationId`:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 16 | operation ID |
| 16 | 32 | complete SHA-256 canonical-intent digest |
| 48 | 32 | opaque stable principal-scope digest |
| 80 | 2 | opcode |
| 82 | 2 | logical ObjectKind or DraftPartKind; zero only when the opcode has no subject |
| 84 | 1 | phase: prepared `1`, draft-open `2`, streaming `3`, sealed `4`, validating `5`, publishing `6`, external-handoff `7`, aborting `8` |
| 85 | 1 | flags: bit 0 resumable; bit 1 draft parent; bit 2 draft child; bit 3 reserved cancellation/recovery slot; bit 4 generation reserved; others zero |
| 86 | 2 | zero |
| 88 | 8 | logical object ID, inactive zero when the opcode has no logical target and valid including zero otherwise; AbortOperation target bytes `0..8` |
| 96 | 8 | expected revision, inactive zero when target mode is not replace and valid including zero otherwise; AbortOperation target bytes `8..16` |
| 104 | 8 | private generation ID; meaningful only when flag bit 4 is set, and then zero is valid |
| 112 | 8 | terminal-commit counter at last durable progress |
| 120 | 4 | latest work-checkpoint sequence, inactive zero without work; zero is valid with work |
| 124 | 1 | AbortOperation reason; zero for every other opcode |
| 125 | 3 | zero |

Eight rows are available to ordinary remote or local claims. The ninth is identified by flag bit 3
and is reserved for one AbortOperation or deterministic local recovery/publication claim, so eight
saturated normal rows cannot make their cancellation structurally impossible. Those maintenance
uses serialize with each other and must leave the reserved row terminal before another begins.
QueryOperation projects storage phases prepared, streaming, sealed, validating, publishing,
external-handoff, draft-open, and aborting as wire phases `0` through `7`, respectively. A local
recovery/publication claim uses validating while checking durable domain evidence and publishing
after it has a complete commit mutation; it does not invent another wire phase. Session attachment
is joined from the coordinator at query time and is not persisted here.

A draft parent is keyed by parent `OperationId`:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 16 | parent operation ID |
| 16 | 32 | BeginDraft intent digest |
| 48 | 8 | private parent-manifest GenerationId; draft state says whether it is present |
| 56 | 2 | final manifest object kind |
| 58 | 2 | declared part count |
| 60 | 1 | state: open `1`, manifest-streaming `2`, finalizing `3`, aborting `4` |
| 61 | 1 | target mode: create `0`, replace `1` |
| 62 | 2 | zero |
| 64 | 8 | target logical ID, zero for create |
| 72 | 8 | expected revision, zero for create |
| 80 | 8 | declared final manifest length |
| 88 | 4 | declared final manifest CRC-32 |
| 92 | 4 | zero |
| 96 | 8 | monotonic DraftRevision |
| 104 | 8 | terminal-commit counter at last durable progress |
| 112 | 4 | latest parent-manifest WORK sequence |
| 116 | 12 | zero |

`BeginDraft` creates this row and durably claims the complete manifest target, compare-and-swap,
length, CRC, and part count. `FinalizeDraft(parent OperationId)` opens or resumes a parent-owned
manifest stream under the already claimed parent operation; the manifest has a private GenerationId
but no DraftPartRef. Finishing that stream validates and publishes. The parent is not an
independently invented `DraftId`.

A draft part is keyed by `(parent OperationId, DraftPartKind, part key)`:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 16 | parent operation ID |
| 16 | 16 | child operation ID |
| 32 | 16 | opaque `DraftPartRef` returned to the client |
| 48 | 2 | DraftPartKind |
| 50 | 2 | zero |
| 52 | 8 | part key |
| 60 | 8 | private generation ID |
| 68 | 8 | payload length |
| 76 | 4 | payload CRC-32 |
| 80 | 1 | state: streaming `1`, sealed `2`, aborted `3` |
| 81 | 15 | zero |

`DraftPartRef` is zero while a part is streaming and is minted at seal with the reversible keyed
codec below. It is an authenticated opaque reference, not an authority-bearing capability.
Clients compare or place it in the matching manifest but cannot derive or choose a `GenerationId`.
Finalizing the parent validates every reference and advances the parent operation in one logical
manifest commit.

The checkpoint's DraftPartRef key is 32 CSPRNG bytes created with StoreId and copied unchanged to
every later checkpoint. It is never exposed outside CardStore. For one sealed part, canonical
context is StoreId `[16]`, parent OperationId `[16]`, DraftPartKind `u16`, part key `u64`, length
`u64`, and payload CRC `u32`. Let `C` be the first eight SHA-256 bytes of ASCII
`O2DR-CHECK\0` followed by that context, interpreted as little-endian `u64`. The plaintext pair is
`(L = GenerationId, R = C)`. Apply six Feistel rounds numbered 0 through 5:

```text
F = little_endian_u64(first_8_bytes(HMAC-SHA256(
      draft_ref_key,
      "O2DR-ROUND\0" || round_u8 || little_endian_u64(R) || context)))
(L, R) = (R, L xor F)
```

DraftPartRef is final `little_endian_u64(L) || little_endian_u64(R)`. Decoding runs the rounds in
reverse and accepts only when the recovered context check equals `C`. The context check gives at
most a 64-bit substitution bound and is not a replacement for principal authentication; all draft
operations already require the parent's authenticated principal. The keyed permutation keeps the
physical GenerationId opaque while letting recovery resolve published manifest children after
terminal draft rows are removed, without a scan, sidecar, or unbounded ref-to-generation table. An
offline attacker who can rewrite card bytes and recompute CRCs is outside this non-adversarial
media-integrity model; DraftPartRef does not turn CRC into authenticity. Store reset changes both
StoreId and key, so old refs fail validation. DOS2 must measure HMAC cost, key residency, decode
scratch, and worst-case 32-reference validation rather than weakening the rounds or check width.

A retained-previous entry is keyed by `GenerationId`: occupied byte `1`, reason flags byte (live
lease bit 0, update rollback bit 1, repository-previous bit 2), lease count `u16`, object kind `u16`, two zero bytes, logical
object ID `u64`, generation ID `u64`, length `u64`, payload CRC `u32`, four zero bytes,
retain-through terminal counter `u64` (`0` means reason-controlled), then 16 zero bytes. Download
leases are RAM ownership facts and disappear on reboot; recovery clears their bit before deciding
reachability. Update rollback reachability persists until handoff reconciliation.

A terminal result is keyed and ordered by its commit sequence. The principal digest is required so
status can be authorized without revealing whether another principal owns the OperationId:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 8 | terminal commit sequence |
| 8 | 16 | operation ID |
| 24 | 32 | canonical-intent digest |
| 56 | 32 | opaque stable principal-scope digest |
| 88 | 1 | terminal state: committed `1`, aborted `2` |
| 89 | 1 | result type: aborted `0`, ObjectResult `1`, DraftPartResult `2`, AbortResult `3`, DomainResult `4` |
| 90 | 2 | encoded length: aborted ErrorBody `48`, or result `64`, `88`, `56`, or `48` respectively |
| 92 | 12 | zero |
| 104 | 88 | exact result or diagnostic-text-free ErrorBody, followed by zero |
| 192 | 16 | zero |

DomainResult body bytes are OperationId `[16]`, StoreId `[16]`, ObjectKind/domain `u16`, outcome
`u16`, domain-state revision `u64`, and reserved zero `u32`, exactly 48 bytes. OBC2 uses outcome
`weatherRequestChanged` `1` and `updateStateChanged` `2`; ride publication remains ObjectResult.
DomainResult is a storage-local result codec and is not a public link ResultEnvelope.

`terminal commit sequence` is the checkpoint's terminal-commit counter after increment, not the
journal sequence. Ring append writes `(result_start + result_count) mod 64`; when already full it
overwrites `result_start` and advances that index by one. This is the only eviction path.

The update-handoff projection uses the `HandoffRef` codec in section 10. Header counts and the
result-ring indices select occupied entries, so an all-zero opaque identity remains valid and is
never repurposed as an empty-slot sentinel.

The one weather-request state has this exact 80-byte codec:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 1 | occupied, exactly `1` |
| 1 | 1 | state: pending `1`, satisfied `2` |
| 2 | 2 | flags: weather head present bit 0; others zero |
| 4 | 8 | WeatherRequestId |
| 12 | 8 | request-context revision |
| 20 | 8 | reserved weather LogicalObjectId |
| 28 | 8 | weather repository Revision captured for response CAS |
| 36 | 4 | required centre latitude, signed degrees times 10,000,000 |
| 40 | 4 | required centre longitude, signed degrees times 10,000,000 |
| 44 | 4 | required radius metres |
| 48 | 4 | zero |
| 52 | 8 | earliest issued UTC, signed Unix seconds |
| 60 | 8 | required valid-until UTC, signed Unix seconds |
| 68 | 8 | head WeatherRequestId; inactive zero only when head-present is clear |
| 76 | 4 | zero |

Signed fields are stored as their little-endian two's-complement bits. Request ID and context
revision never wrap. A device-local weather-context change uses the local-producer principal and a
durable claim in the reserved ninth row. Its OperationId is the first 16 SHA-256 bytes of
ASCII `O2-LOCAL-WX-ID\0`, StoreId, new WeatherRequestId `u64`, and new context revision `u64`.
Canonical intent is full SHA-256 over ASCII `O2-LOCAL-WX-INTENT\0`, StoreId, and the exact new
80-byte WeatherState encoding. The terminal record atomically puts this state and appends
DomainResult outcome `weatherRequestChanged` with domain-state revision equal to the new
request-context revision; it does not change the weather object repository
Revision. Publishing a bundle always puts the resulting weather state in the same terminal record
as the catalog head, repository Revision, and ObjectResult: a current-request publication sets
`satisfied` and its head request ID. A useful superseded publication is admitted only when there is
no weather head or its validated issued UTC is strictly newer than the head's, while all
current-context and catalog-CAS checks still apply; it updates the head request ID but leaves the
current request pending. No reboot can expose the new head with the old request state.

Deleting the weather head uses one terminal record that removes the catalog head, advances the
weather repository Revision, appends the delete ObjectResult, and puts WeatherState with
head-present clear, the inactive head request ID zeroed, and context state pending. It preserves
the WeatherRequestId, request-context revision, singleton identity, and requested coverage/time
facts. The gate therefore exposes either the old head with its old state or no head with the same
request pending; QueryWeatherRequest cannot observe a stale head fact after deletion.

The one active-ride state has this exact 128-byte codec:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 1 | occupied, exactly `1` |
| 1 | 1 | state: recording `1`, stopping `2`, sealed `3`, claimed `4`, recovery-fault `5` |
| 2 | 1 | flags: historical route snapshot present bit 0; others zero |
| 3 | 5 | zero |
| 8 | 8 | ride-recovery revision, equal to the initial domain journal sequence |
| 16 | 16 | CSPRNG local publication OperationId |
| 32 | 32 | device-local ride-producer principal digest |
| 64 | 8 | prospective ride GenerationId |
| 72 | 8 | start UTC, signed Unix seconds |
| 80 | 8 | historical route LogicalObjectId; inactive zero when flag bit 0 is clear and valid including zero when set |
| 88 | 8 | historical route Revision; inactive zero when flag bit 0 is clear and valid including zero when set |
| 96 | 32 | zero; payload progress and seal facts are authoritative only in RIDE.ACT |

The route pair is a historical start-of-ride snapshot, not a lease or mutable relationship. The
ride-recovery revision is the globally unique sequence of the initial domain record and remains
fixed for that ride. This projection
is authoritative for existence, identity, and lifecycle state; RIDE.ACT is authoritative for
payload progress and seal facts. Ride recovery remains UI-neutral until ordinary ride publication
creates a catalog head.

## 6. Commit journal

`COMMIT.JNL` is preallocated to 524,288 bytes: 256 slots of 2,048 bytes. Each slot is a 1,536-byte
body and a 512-byte `O2JG` gate. A slot is written once in an epoch. Body bytes `1532..1536` hold
the body CRC.

### 6.1 Journal body

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `O2JR` |
| 4 | 2 | format version `1` |
| 6 | 2 | header length `96` |
| 8 | 16 | StoreId |
| 24 | 8 | epoch |
| 32 | 8 | globally contiguous sequence |
| 40 | 2 | physical slot index |
| 42 | 2 | record kind: claim `1`, work `2`, terminal `3`, retention `4`, handoff `5`, domain `6` |
| 44 | 16 | operation ID, zero only for retention, pre-claim ride recovery, or completed-handoff cleanup |
| 60 | 32 | canonical-intent digest, zero only for retention, pre-claim ride recovery, or completed-handoff cleanup |
| 92 | 2 | mutation length, exactly `1304` |
| 94 | 2 | zero |
| 96 | 1304 | mutation |
| 1400 | 132 | zero |
| 1532 | 4 | body CRC-32 |

The fixed mutation is a compact projection delta, not a union of domain payloads:

| Mutation offset | Size | Field |
| --: | --: | :-- |
| 0 | 2 | mutation version `1` |
| 2 | 2 | zero |
| 4 | 4 | presence flags, below |
| 8 | 2 | repository kind or zero |
| 10 | 2 | record kind, equal to header |
| 12 | 8 | new repository revision or zero |
| 20 | 8 | next logical-ID candidate or zero |
| 28 | 2 | repository flags; logical-ID exhausted bit 0 |
| 30 | 2 | zero |
| 32 | 8 | new next-GenerationId cursor or zero |
| 40 | 128 | active-operation entry |
| 168 | 192 | catalog-head entry |
| 360 | 128 | draft-parent entry |
| 488 | 96 | draft-part entry |
| 584 | 64 | retained-previous entry |
| 648 | 208 | terminal-result entry |
| 856 | 240 | update-handoff entry |
| 1096 | 80 | weather-request state |
| 1176 | 128 | active-ride state |

Presence flags are: active put bit 0, active remove bit 1, head put bit 2, head remove bit 3,
draft-parent put bit 4, draft-parent remove bit 5, draft-part put bit 6, draft-part remove bit 7,
previous put bit 8, previous remove bit 9, result append bit 10, handoff put bit 11, handoff remove
bit 12, and repository revision set bit 13. Put and remove for the same entry are mutually
exclusive. Repository logical-ID cursor set is bit 14, weather-state put bit 15, active-ride put bit
16, active-ride remove bit 17, and next-GenerationId cursor set bit 18. Bits 19..31 are zero. An
absent fixed entry is all zero. Removal keys use their normal key fields and require every non-key
byte to be zero.

When bit 18 is set, the encoded cursor must be the current cursor plus one without wrap. The
record reserves the former cursor value as its GenerationId. A normal claim carries that value in
an active entry with flag bit 4; a pre-claim ride domain record carries it in ActiveRideState; and
an update rollback-snapshot reservation carries it in the already-active install entry. No other
record may set bit 18. This makes reservation of GenerationId zero explicit and prevents replay
from deriving allocation state from rows that a later terminal record removes.

A claim record requires active put and forbids active remove, result append, and head mutation; it
may atomically put the newly reserved draft row. A work record requires active put for an existing
claim and may update its matching draft row, but forbids result and head mutation. A terminal
record requires active remove and result append and may contain the publication fields. A
retention record has zero OperationId/digest and changes exactly one previous entry. A handoff
record changes the one handoff entry and may update the already-active install operation; the
install-requested terminal record is still record kind terminal. A handoff-remove record with zero
OperationId/digest is valid only for the bounded cleanup suffix of a selected `complete` handoff
when neither its install claim nor a post-boot local claim remains active. A domain record has zero
OperationId/digest and changes only the single active-ride recovery state before its publication
claim, setting the next-GenerationId cursor only on initial reservation. Weather state changes only
in the terminal record of its claimed local operation or weather-object publication. Any other
combination is invalid.

One terminal record may atomically remove the active operation, change one logical head, retain its
previous generation, update one repository revision, remove a draft parent, append the result, and
update the handoff, weather, or active-ride projection. Removing a terminal draft parent also
removes every draft-part row with that parent in the same replay step. Terminal draft status comes
only from its retained QueryOperation result; QueryDraft never depends on a finalized/aborted row.
Sealing a child removes its active-operation row but retains one sealed draft-membership row until
the parent terminates, because parent validation still needs it.

### 6.2 Claim point and exactly-once result

The following failures do **not** claim an `OperationId`: malformed framing, failed
authentication/authorization, unsupported opcode or version, an illegal identity, unknown
mandatory fields, noncanonical metadata, inability to compute the complete canonical intent, a
transient owner/recovery/power condition, or the resource/space preflight in section 11. They
return an immediate error and `QueryOperation` remains `Unknown`.

After those checks and preflight, the next durable act is a journal claim record containing the
operation ID, full intent digest, principal-scope digest, and active-operation entry. The same
record atomically reserves the logical/singleton target, generation or draft slot, and logical
space accounted by preflight; there is no claim-without-reservation crash window. Revision and
semantic validation failures discovered after claim append a terminal `Aborted` record and remove
the active operation. A caller may retry transport delivery of the same intent and ID, but must use
a new `OperationId` for a new semantic attempt after a terminal abort. This ordering prevents an
authenticated, admitted operation from disappearing merely because reset occurred during
validation.

An existing active or retained terminal operation with the same full digest returns its current
state/result. A different digest is `operationIdConflict`. For an operation entering through the
claim coordinator, no payload file, work record, draft row, or domain mutation is created before
the claim gate is durable. The pre-claim ride recorder in section 7.1 is instead a bounded local
domain state and becomes a queryable operation only after seal. If the journal itself cannot be
made writable, no claim can be guaranteed; the response is an uncertain storage failure and the
client queries before choosing any further action.

A logical mutation becomes committed only at the terminal journal gate containing its new
repository revision, head/draft/handoff transition, and complete terminal result. Payload bytes are
sealed and synchronized first. In-memory state changes only after the gate sync. Thus a cut before
the gate recovers the old head and active operation; a cut after it recovers the new head and
result, even when the response was never sent.

AbortOperation is a cancellation operation, not a session teardown. After owner/intent lookup and
preflight, it claims the reserved ninth row with phase `aborting`, stores the target OperationId
verbatim in bytes `88..104` and the reason at byte 124, and then follows this exact sequence:

1. If the target is already terminal or authorized-absent, skip target mutation and remember that
   deterministic AbortResult disposition.
2. For an ordinary active target, append its Aborted terminal result and remove its active row in
   one record. Only after that gate may its WORK/payload become collectible.
3. For a draft parent, atomically set the target active phase to `aborting` and its parent state
   to `aborting`, which immediately forbids new parts and finalization. For each nonterminal child
   in key order, append one record that removes its active row, appends its Aborted result, and puts
   its draft-part row in `aborted`. Then append the parent's Aborted terminal result, remove its
   active/parent rows, and implicitly remove all child membership rows.
4. Append the abort command's AbortResult and remove the reserved claim in one terminal record.

Each record carries only one terminal result, so a cut resumes at the first missing suffix. The
claimed command's stored target and reason make that suffix deterministic without inspecting freed
sessions or guessing from files. QueryOperation reports the abort command—and a draft parent after
step 3 begins—as InProgress phase `aborting` until their respective terminal records commit.

### 6.3 Replay and compaction

Recovery chooses the structurally valid checkpoint with the greatest `through_sequence`; differing
valid checkpoints at the same sequence are corruption. It replays only journal records whose
StoreId and epoch match and whose sequences begin exactly at `through_sequence + 1`. Replay stops
at the first missing, invalid, torn, duplicate, wrong-slot, or noncontiguous record and ignores all
later bytes. Two differing valid records claiming the same sequence are corruption, not a choice.
Within an epoch, physical journal slot `i` must carry sequence `checkpoint through_sequence + i +
1`; another mapping is invalid even when its CRCs pass.

Before accepting a 193rd record in one epoch, `CardStore` blocks new mutations and compacts:

1. Apply all valid records through sequence `S` in memory.
2. Invalidate and sync the inactive checkpoint gate.
3. Write its complete body with epoch `E + 1` and through-sequence `S`; sync.
4. Write and sync its `O2CG` gate.
5. Only now write sequence `S + 1`, epoch `E + 1`, at journal slot zero.

A cut before step 4 recovers the old checkpoint and old-epoch journal. A cut after step 4 recovers
the new checkpoint and ignores every old-epoch slot. Journal slots need no erase transaction and
are reusable only because their epoch no longer matches the selected checkpoint. The 64 unused
slots above the trigger are recovery headroom, not permission to continue indefinitely after a
failed compaction.

## 7. Work record and payload ordering

Every generation managed by a resumable object, manifest, part, or update rollback-snapshot copy
has a 2,048-byte `WORK` file: two alternating 1,024-byte slots, each a 512-byte body plus `O2WG`
gate. An active ride is the sole exception and uses section 7.1 instead. The gate's physical slot
is 0 or 1, scope is `GenerationId`, and logical sequence is the work-checkpoint sequence. The body
CRC is at `508..512`.

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `O2WK` |
| 4 | 2 | format version `1` |
| 6 | 2 | header length `176` |
| 8 | 16 | StoreId |
| 24 | 16 | operation ID: child for a part, parent for its manifest, or ordinary operation |
| 40 | 32 | canonical-intent digest |
| 72 | 16 | parent operation ID for a draft child, valid including all zero; inactive zero otherwise |
| 88 | 16 | opaque DraftPartRef after child seal, valid including all zero; inactive zero otherwise |
| 104 | 8 | private GenerationId |
| 112 | 8 | declared length |
| 120 | 4 | declared payload CRC-32 |
| 124 | 1 | state: streaming `1`, sealed `2` |
| 125 | 1 | flags: bit 0 resumable; others zero |
| 126 | 2 | zero |
| 128 | 8 | durable next offset |
| 136 | 4 | finalized CRC-32/IEEE through durable next offset |
| 140 | 4 | work-checkpoint sequence |
| 144 | 8 | terminal-commit counter at last durable progress |
| 152 | 2 | logical ObjectKind or DraftPartKind |
| 154 | 1 | subject namespace: logical object `1`, draft part `2` |
| 155 | 1 | zero |
| 156 | 8 | draft part key or zero |
| 164 | 4 | observed payload file length |
| 168 | 8 | zero |
| 176 | 332 | zero |
| 508 | 4 | body CRC-32 |

`BeginWork` reserves the next GenerationId and the preflighted logical resources in the catalog
journal before either physical file is created. Recovery can therefore recreate missing files at
offset zero without reusing the ID.
Payload and WORK files are preflighted and preallocated as far as the FAT adapter supports, but
logical free-space reservation remains owned by CardStore even where FAT allocation is lazy.

For each acknowledged checkpoint:

1. Write payload bytes at the current durable offset; bytes beyond it from an earlier torn attempt
   are overwritten, never trusted.
2. Synchronize the payload and obtain its observed file length.
3. Invalidate and synchronize the older WORK gate.
4. Write the inactive WORK body with the new offset, finalized prefix CRC, observed length, and sequence;
   synchronize.
5. Write and synchronize its gate. Only then acknowledge the offset.

The durable offset cannot exceed the declared or observed length. Resume recomputes or verifies
the finalized CRC through that offset before accepting more bytes; an implementation may invert
the final XOR to restore its internal accumulator. Seal requires exact length and
whole-object CRC, synchronizes and closes the payload, writes a sealed WORK slot, and only then
allows domain validation. A sealed generation is immutable. The terminal journal record wins over
stale WORK state; a work record can never resurrect a terminal operation.

### 7.1 Active ride recovery journal

`RIDE.ACT` is preallocated to 16,384 bytes and contains 16 circular slots. Each slot is a 512-byte
body followed by an `O2RG` gate. The gate's physical index is `0..15`, scope is the prospective ride
GenerationId, and logical sequence is the ride checkpoint sequence. Body CRC is at `508..512`.

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `O2RA` |
| 4 | 2 | format version `1` |
| 6 | 2 | header length `136` |
| 8 | 16 | StoreId |
| 24 | 8 | ride-recovery revision |
| 32 | 16 | local publication OperationId |
| 48 | 8 | prospective GenerationId |
| 56 | 1 | recovery evidence state: recording `1`, stopping `2`, sealed `3` |
| 57 | 1 | flags: historical route snapshot present bit 0; others zero |
| 58 | 6 | zero |
| 64 | 8 | start UTC, signed Unix seconds |
| 72 | 8 | historical route LogicalObjectId; inactive zero when flag is clear and valid including zero when set |
| 80 | 8 | historical route Revision; inactive zero when flag is clear and valid including zero when set |
| 88 | 8 | durable payload offset |
| 96 | 4 | finalized CRC-32 through durable offset |
| 100 | 4 | ride checkpoint sequence |
| 104 | 8 | durable sample count |
| 112 | 8 | durable elapsed milliseconds |
| 120 | 8 | sealed length; inactive zero before seal, and zero is valid when sealed |
| 128 | 4 | sealed CRC-32; inactive zero before seal, and zero is valid when sealed |
| 132 | 4 | zero |
| 136 | 372 | zero |
| 508 | 4 | body CRC-32 |

Exactly one ride may be recording **or recoverable**. Starting another ride while the checkpoint's
active-ride count is one, or while recovery has not reconciled a matching valid RIDE slot, is
refused; new recording never truncates recovery evidence. Start generates a fresh local
OperationId, reserves a GenerationId, and puts initial ActiveRideState in one domain journal
record before creating the GEN payload. The OperationId is recovery identity at this stage, not a
claimed QueryOperation entry.

Each ride checkpoint first writes and synchronizes payload bytes. It then invalidates and syncs
slot `checkpoint_sequence mod 16`, writes and syncs that body, and writes and syncs its gate. The
previous highest valid slot remains authoritative until the new gate is durable. Recovery accepts
only the greatest valid sequence matching the checkpoint ActiveRideState's StoreId, OperationId,
GenerationId, and recovery revision; equal-sequence differing bodies are corruption. A RIDE file
alone cannot invent an active ride. ActiveRideState never copies a newer RIDE offset or CRC, so no
journal write is required for an ordinary recording checkpoint. A recording state with no matching
slot is the initial durable offset zero; recovery recreates or truncates the GEN payload to zero,
so payload bytes synchronized before a torn first RIDE gate are not mistaken for acknowledged data.

Stop first journals ActiveRideState `stopping`, then writes the final payload checkpoint and
validates exact ride bytes. It next writes a `sealed` RIDE slot with final length/CRC and finally
journals ActiveRideState `sealed`. A cut before the stopping gate resumes recording; a cut after it
finishes stopping from the previous or new RIDE prefix. A cut after the sealed RIDE gate but before
the sealed state gate validates that slot and completes the state transition. The state can never
be `sealed` without a matching valid sealed slot.

The local ride producer next preflights an ordinary ride create and claims its stored OperationId
in the reserved ninth active row with phase `validating`. Its canonical intent is the normal
StartUpload create intent for ObjectKind ride,
the sealed length/CRC, and the empty ride Put-v1 envelope. The claim atomically assigns the
LogicalObjectId and changes ActiveRideState to `claimed`. Publication uses an ordinary ObjectResult
and one terminal record that puts the ride head, advances its repository Revision, removes the
active claim and ActiveRideState, and appends the result. Only that gate makes the ride visible.

A cut resumes from the greatest valid suffix: recording/stopping resumes or remains recoverable;
sealed performs the same durable local claim; claimed resumes through
QueryOperation state; terminal publication wins over stale RIDE slots. Explicit discard before
claim removes ActiveRideState durably before GC. After claim it uses the normal Aborted terminal
path. Recovery need not synthesize an in-progress UI ride, and the optional route snapshot never
pins or resurrects the route. DOS2 must measure checkpoint cadence, 16-slot wear distribution,
payload-sync latency, and resident recovery state on target.

## 8. Draft publication

`BeginDraft` claims the parent OperationId and complete manifest intent before accepting parts.
Each `StartDraftPart` independently claims its child OperationId. Sealing mints one opaque
DraftPartRef and commits it with the child terminal result and draft-part projection in one journal
record, but creates no logical catalog head. A reset at any byte cut therefore exposes either
resumable child work or one sealed part, never an unnamed published object.

`FinalizeDraft(parent OperationId)` starts or resumes the parent-owned manifest upload using the
length and CRC already bound by BeginDraft. `FinishUpload` rechecks the parent's expected repository
revision under the catalog commit lock, validates exactly the declared number of unique child
refs, and verifies their parent, part kind, key, length, and CRC against the manifest payload. Its
single terminal record publishes the manifest head, advances the repository revision, retains an
old leased head when necessary, records the parent result, and removes the parent plus every child
membership row in the same replay step. Only that gate makes the manifest and all its children
reachable to normal readers. Initial publication derives catalog selected as false; selection is a
later compare-and-swap SetMetadata mutation. QueryDraft is then unknown; the retained parent
QueryOperation result is the sole terminal-status projection.

Aborting or expiring a parent first enters `aborting`. Recovery durably aborts each still-active
child in bounded individual commits, then terminally aborts the parent and makes all of its draft
rows logically free. A cut resumes at the first nonterminal child. A terminal child result remains
in the same 64-entry global ring; cleanup does not create compatibility aliases or companion
logical objects.

## 9. Leases, retention, and garbage collection

A download lease is a RAM-only capability containing `(connection generation, SessionId,
GenerationId)`. Acquisition pins the resolved generation before acceptance. Only exact capability
equality can advance or release it; a stale disconnect or reused numeric SessionId from another
connection is a no-op. Four leases may coexist. Reset closes every connection and lease, so no
lease record is replayed from media.

Replacing or deleting a head moves that immutable generation into the retained-previous table in
the same terminal commit. It becomes the one repository-previous generation for that ObjectKind;
a later displacement removes that reason from the older entry after publication, while any lease
or update reason continues to retain it. Admission reserves enough capacity for the new entry and
rejects a mutation that would need a seventeenth entry. Release removes only the lease reason
through a retention journal record and never changes the newer head. Update rollback and handoff
reasons are removed only by update reconciliation.

Reachability is computed from catalog heads, transitive children resolved from published manifests'
validated DraftPartRefs, open draft parents and sealed parts, active operations and WORK records,
ActiveRideState and its matching RIDE slot, retained previous entries, the current update handoff,
and live leases. GC processes at most one generation per invocation,
recomputes reachability under the CardStore lock immediately before deletion, and stops on an
unknown record or path. Deleting an unreachable GEN/WORK pair may be interrupted at either file;
both orderings recover as harmless orphan cleanup because no catalog fact points to it. Publication
never waits for deletion and never edits an old generation.

## 10. Update A/B handoff

Update upload publishes only a validated `VerifiedReady` package. `InstallUpdate` is a separate
authenticated operation. OBC2 uses `ARM0.HND` and `ARM1.HND` to bind that operation to the existing
OBCU boot-state page without requiring FAT support in the bootloader.

Each ARM file is a 512-byte body plus an `O2HG` gate. The body CRC is at `508..512`; its gate uses
physical slot 0 or 1, scope equal to `handoff_sequence`, and logical sequence equal to the encoded
phase value. The body is:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `O2UH` |
| 4 | 2 | format version `1` |
| 6 | 2 | header length `64` |
| 8 | 16 | StoreId |
| 24 | 8 | handoff sequence |
| 32 | 2 | HandoffRef length, `240` |
| 34 | 30 | zero |
| 64 | 240 | HandoffRef |
| 304 | 204 | zero |
| 508 | 4 | body CRC-32 |

`HandoffRef` has this exact codec:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 8 | handoff sequence |
| 8 | 1 | phase: prepared `1`, armed `2`, trial-observed `3`, outcome-observed `4`, complete `5` |
| 9 | 1 | OBCU outcome: none `0`, installed `1`, rolled-back `2`, stage-rejected `3`, arm-abandoned `4` |
| 10 | 2 | flags: rollback snapshot present bit 0; other bits zero |
| 12 | 4 | zero |
| 16 | 16 | InstallUpdate OperationId |
| 32 | 32 | canonical-intent digest |
| 64 | 8 | package GenerationId |
| 72 | 8 | package length |
| 80 | 4 | package payload CRC-32 |
| 84 | 4 | nonzero OBCU arm generation |
| 88 | 32 | SHA-256 of the exact encoded OBCU Armed blob, including its CRC |
| 120 | 64 | staged package OBCU ImageHeader |
| 184 | 8 | terminal-result commit sequence, zero until committed |
| 192 | 4 | observed OBCU outcome generation, zero until observed |
| 196 | 4 | zero |
| 200 | 8 | private rollback-snapshot GenerationId; inactive zero when flag bit 0 is clear, and zero is valid when set |
| 208 | 8 | rollback-snapshot length; inactive zero when flag bit 0 is clear |
| 216 | 4 | rollback-snapshot CRC-32; inactive zero when flag bit 0 is clear |
| 220 | 4 | zero |
| 224 | 8 | update-package LogicalObjectId |
| 232 | 8 | latest update repository Revision represented by this handoff |

The HandoffRef sequence must equal the outer body and gate scope, and its phase must equal the gate
logical sequence; a mismatch invalidates the ARM record.

Handoff sequence is store-global, nonzero, and never wraps. Phases advance strictly in their
numeric order and a phase value is written at most once for one handoff. Recovery selects the valid
ARM file with the lexicographically greatest `(handoff_sequence, phase)` pair. Equal pairs with
differing valid bytes are corruption. Before writing the next phase, the writer invalidates and
synchronizes the gate of the older/inactive ARM file; the currently selected file remains valid
until the replacement gate is durable. Thus a cut during a phase advance selects the old pair or
the strictly greater new pair, never an ambiguous equal-sequence tie.

The current OBCU specification remains the authority for `OBCB` boot-page bytes, `StagedRef`
extent bounds, signature checks, Trial confirmation, watchdog, and rollback. DOS v2 writes its
current format-2 Armed page and does not translate an earlier DOS card format. The OBCU boot page
is internal RRAM, not part of `/OBC2`.

The arming protocol is:

1. Durably claim InstallUpdate. Revalidate authorization, signature, digest, target, downgrade,
   size, battery, and runtime safety. When rollback is possible, snapshot the running slot into a
   sealed private OBC2 generation. Resolve the immutable package and optional snapshot to OBCU
   extents and build the exact Armed blob with a fresh nonzero arm generation.
2. Invalidate the older ARM gate, then write and synchronize a `prepared` HandoffRef in the other
   ARM file. Its reachability pins the package and rollback snapshot.
3. Write the complete OBCU Armed page in whole RRAM lines, issue the hardware persistence barrier,
   read it back, decode it, and require its arm generation and full-blob SHA-256 to match.
4. Write and synchronize an `armed` HandoffRef to the alternate ARM file.
5. Append the terminal journal record containing the install-requested ObjectResult, the update
   head's `installRequested` catalog metadata and new repository Revision, and the armed handoff
   projection carrying that Revision. Only its gate permits a success response.
6. Drain transport best-effort and reset. The bootloader independently revalidates, installs,
   enters Trial, and rolls back an unconfirmed trial according to OBCU.

Only one handoff may be prepared or armed. A second InstallUpdate is terminally aborted as busy.
The application never clears or re-arms merely because a response was lost.

### 10.1 Recovery at every cut

| Durable facts after a cut | Recovery action | QueryOperation |
| :-- | :-- | :-- |
| no claim | no install work exists | `Unknown` |
| claim only | resume validation or terminally abort; do not arm from guessed files | `InProgress` until resolved |
| prepared ARM, write/readback mismatch proven in the same boot epoch | rebuild from the pinned package and retry step 3 without resetting | `InProgress` |
| prepared ARM, matching Armed/Trial page | write armed ARM and terminal result; never create a second arm generation | `InProgress`, then committed |
| prepared ARM, matching Idle outcome | persist outcome-observed then complete ARM; append the missing install-requested result bound to that projection; run the post-boot local-state suffix; never re-arm | committed after reconciliation |
| armed ARM, terminal journal absent | with Armed/Trial append the same install-requested result; with matching Idle first persist outcome-observed then complete ARM and bind that projection in the result, then run the post-boot suffix | `InProgress`, then committed |
| outcome-observed ARM, install terminal absent | persist complete ARM, append the same install-requested result bound to the selected complete projection, then run the post-boot suffix | `InProgress`, then committed |
| complete ARM, install terminal absent | append the same install-requested result bound to that projection, then run the post-boot suffix | `InProgress`, then committed |
| terminal journal durable, response/reset lost | return retained result and proceed with one orderly reset if still Armed | committed |
| Trial page | leave installation to OBCU confirmation/rollback rules; keep package and rollback reachable | committed |
| matching Idle outcome | write and sync outcome-observed then complete ARM records; commit the post-boot local state; run the retention-clear/handoff-remove suffix | committed |
| nonmatching valid boot generation/outcome | mount update installation degraded and require explicit recovery; never re-arm or delete evidence | committed if terminal exists, otherwise in progress |
| prepared ARM after a reset, but no matching Armed, Trial, or Idle outcome | do not re-arm or collect sources; flash progress is unknowable and this is a NO-GO fault | `InProgress` |
| torn/unknown boot page with evidence flash may have begun | do not rewrite the page or collect sources; boot recovery is a NO-GO fault | committed if its terminal record exists, otherwise `InProgress` |

The exact Armed-blob hash and arm generation disambiguate a cut after the RRAM write from an
unrelated boot state. `InstallUpdate` commits the request to install, not the eventual trial
verdict; a later rollback does not rewrite its terminal result. Each later state is a separate
device-local producer operation.

### 10.2 Post-boot update state operations

The update head transitions from `installRequested` to `trial`, then to `confirmed`, or directly
to `rolledBack`/`failed` when that is the first recoverable post-boot fact. A valid OBCU Trial page
maps to trial; matching Idle outcomes Installed, RolledBack, and StageRejected/ArmAbandoned map to
confirmed, rolledBack, and failed respectively. The application must commit observed trial before
writing its health confirmation to OBCU Idle.

For target state byte `S`, the local OperationId is the first 16 bytes of SHA-256 over exact bytes
ASCII `O2-LOCAL-UPD-ID\0`, StoreId `[16]`, handoff sequence `u64`, update LogicalObjectId `u64`,
OBCU arm/outcome generation `u32`, and `S u8`. Its full intent digest is SHA-256 over ASCII
`O2-LOCAL-UPD-INTENT\0` followed by those same fields and the currently expected update repository
Revision `u64`. A cryptographic collision with a different retained/active operation is a recovery
fault; recovery never chooses another ID. The principal scope is the fixed device-local update
producer.

Reconciliation uses the reserved ninth active row and is exact:

1. Validate the OBCU fact against the selected handoff's arm generation, package header, and
   current update head. Durably claim the deterministic local operation at phase `validating`.
2. For trial, write and sync `trial-observed` to the inactive ARM file. For a terminal OBCU outcome,
   write and sync `outcome-observed`, then `complete`, through alternating ARM files.
3. Append one terminal record that puts the unchanged payload generation/length/CRC with catalog
   metadata state `S`, increments and stores the update repository Revision, puts the matching
   handoff projection and Revision, removes the local active row, and appends DomainResult outcome
   `updateStateChanged` with domain-state revision equal to that new repository Revision.
4. After a complete outcome terminal record, when the handoff's rollback-present flag is set,
   append a zero-identity retention record that clears the rollback bit from its one
   retained-previous entry, removing the entry only when no reason remains. The still-present
   handoff projection keeps the snapshot reachable across this cut. Then append a zero-identity
   handoff record that removes the `complete` projection. With no rollback snapshot, skip directly
   to that handoff removal. Only after its gate may an otherwise unreferenced snapshot be collected.
   The completed ARM record remains bounded diagnostic evidence.

A cut before the local claim leaves only the external OBCU fact, from which the same ID is derived.
A cut after claim reports QueryOperation phase `validating`. A cut after an ARM gate but before
the terminal catalog gate resumes the same claim. A cut after the terminal gate exposes the new
head metadata, repository Revision, handoff phase, DomainResult, and CommitEvent catch-up together;
it can expose none of them partially. Live code emits CommitEvent only after applying that gate,
and reboot consumers obtain the same coalesced catch-up from the recovered repository Revision—no
durable forwarding queue or duplicate catalog mutation exists.

A cut in the two-record cleanup suffix resumes from the selected complete ARM plus the catalog
projection: rollback reason present means run both records; reason absent with projection present
means run only the handoff removal. A missing projection means cleanup is complete. These records
never change the update head, its Revision, or either terminal operation result.

Completed ARM files never extend QueryOperation retention: only the active table and 64-result ring
answer it. Their exact facts permit deterministic catalog reconciliation after the corresponding
DomainResult is evicted, but do not authorize replay of that old OperationId.

## 11. Admission and resource preflight

Preflight runs under CardStore's admission lock before durable operation claim and before a
generation, draft row, lease, or file handle is acquired. It uses checked `u64` arithmetic and a
single resource plan. Failure returns without claiming the OperationId or partially allocating
resources. A successful claim atomically consumes the planned logical slots/reservations.

The plan proves all applicable conditions:

- the all-kind and per-kind head limits after the proposed mutation;
- one of eight normal active-operation slots, or the one reserved cancellation/recovery row for an
  eligible operation; for resumable upload one of four durable WORK slots; and no other attached
  heavy stream session;
- no existing active or recoverable ride state before starting a ride;
- at most two draft parents, 32 total parts, 32 refs in the manifest, and 11 simultaneously mounted
  selected map files on this board;
- at most four reader leases and 16 retained previous generations after publication;
- a free journal slot, with successful compaction first when the trigger is reached;
- declared generation length at most `0xFFFF_FFFF`, representable file offsets, and enough free FAT
  clusters for all unfulfilled logical reservations plus payload, WORK file, directory growth, and
  eight safety clusters;
- enough transient FAT handles for the complete operation at its worst step, including payload,
  WORK, journal/checkpoint, and any already pinned readers or mounted map files;
- no prepared/armed update handoff, and enough OBCU extents, app-slot space, rollback resources,
  and one of the four WORK slots when a rollback snapshot is required, for InstallUpdate.

Free space advertised to clients already subtracts outstanding reservations and eight filesystem
safety clusters. Outside modification while mounted is unsupported; an observed allocation or
directory change that invalidates a reservation fails closed and triggers recovery, not
overcommitment. Domain validators may run outside the commit lock, but expected revision and the
entire resource plan are rechecked under that lock immediately before the journal body write.

## 12. Initialization, recovery classes, and media cuts

Absence of `OBC2` is the normal fresh-card state. Initialization creates entries in this fixed
order: the `OBC2` directory; preallocated `INIT.REC`; its valid witness gate; `GEN` and its shards
in numeric order; `WORK` and its shards; COMMIT, ARM0, ARM1, RIDE, CAT0, and CAT1; then the first
checkpoint. It generates all 128 StoreId bits and the 32-byte DraftPartRef key with a CSPRNG and
writes the incomplete-initialization witness using a 512-byte body and `O2IG` gate.
The body is magic `O2IN` at 0, version `1` at 4, header length `32` at 6, StoreId at 8, 484 zero
bytes, and body CRC at 508. Its slot index is zero; StoreId bytes `0..8` and `8..16` are copied
verbatim into the gate's scope and sequence fields solely to bind the two records.
The private DraftPartRef key is not copied into INIT. If initialization resumes before StoreId
birth it generates a new key, which is safe because no valid checkpoint, draft, or external ref
has existed.
It then creates both role trees, preallocates journal/ARM/checkpoint/RIDE files, and writes the
first CAT0 checkpoint with epoch 1, through-sequence 0, next GenerationId 0, and terminal counter
0. That checkpoint reserves weather LogicalObjectId zero by setting the weather repository's next
candidate to one while leaving weather-state count zero; zero is an ordinary allocated value, not
an absence sentinel. Initialization finally deletes `INIT.REC` and synchronizes the directory. The
first checkpoint gate is the StoreId birth point; it is never advertised earlier.

Before the first checkpoint gate, StoreId has never escaped CardStore. If reset leaves no valid
INIT or checkpoint, automatic restart is allowed only when the directory entries are an exact
prefix of the creation order above, every present name has the specified type and maximum
preallocated length, and no present slot has any valid OBC2 gate. Recovery removes that unowned
prefix, synchronizes its parent directory, and restarts with a new StoreId and key. An unknown name,
oversize entry, or valid gate is not a pre-birth prefix and fails closed. With a valid INIT but no
checkpoint, recovery preserves its StoreId, truncates or completes only the same ordered
preallocation prefix, and resumes initialization. Thus every cut before birth is either a bounded
restart/resume case or explicit corruption; it never silently reformats an advertised store.

On mount:

- no `OBC2`: initialize;
- a valid checkpoint: mount it, even if a stale INIT record remains, then replay;
- no valid checkpoint but one valid INIT record and only its exact ordered preallocation prefix:
  resume initialization with that unadvertised StoreId;
- no valid checkpoint or INIT but an exact ungated pre-birth prefix: remove it and restart as
  specified above;
- any other nonempty or unknown OBC2 shape: mount recovery-failed/read-only;
- valid metadata that references a missing/torn generation: mount degraded/read-only and preserve
  evidence;
- terminal result plus stale active/WORK data: terminal result wins and stale bytes are GC input;
- valid resumable work: expose it only through the matching OperationId and intent digest;
- unknown private magic/version or equal-sequence differing records: corruption, never guess or
  delete.

The required cut tests cover every sector boundary and every sync return before and after: StoreId
birth; journal claim gate; generation reservation; each payload/WORK checkpoint; seal; terminal
catalog/result gate; checkpoint compaction gate; lease-preserving replace/delete; draft-part seal
and parent finalization; weather-context claim/publication/delete; every RIDE.ACT/ActiveRideState
start/checkpoint/stop/seal/claim/publish/discard transition; ARM A/B preparation; OBCU page
write/readback; armed handoff; install terminal result; and every post-boot local claim,
trial/outcome/complete ARM write, terminal state commit, rollback-retention clear, and handoff
removal. Each recovered image must produce exactly the old state, the new state, or the explicitly
listed in-progress state—never a mixed head and result, reused ID, leaked draft, released foreign
lease, or automatic reformat.

## 13. Current embedded adapter facts and measurement status

The nRF54L adapter currently configures four directory handles and 16 file handles. Its measured
`embedded_sdmmc` `FileInfo` is 64 bytes at alignment four; increasing the historic six-handle
budget to 16 cost 640 bytes of `.bss`. Existing ride/upload activity consumes a practical peak of
five handles, leaving 11 for a mounted map set. These are measured adapter facts and explain the
board-specific mount limit; they do not widen the portable CardStore API.

The adapter's seek offsets and file lengths are `u32`, hence the `0xFFFF_FFFF` single-generation
limit even though the wire uses `u64`. Larger logical releases use bounded manifests. The fixed
four-level directory tree and sequentially reopened metadata files avoid requiring more directory
handles or permanently holding journal/checkpoint handles.

The checkpoint, journal, WORK, result, and capacity sizes in this document are contract constants,
not measurements of latency, stack, heap, boot time, wear, or worst-case card fragmentation. DOS2
must report those measurements for the target profile and may reduce runtime concurrency or reject
admission when the advertised contract permits it. It may not silently change byte layouts,
retention guarantees, identity rules, durability points, or recovery behavior to meet a budget.
