# Flat store seam and protocol v4

- Status: **normative** for the flat store's public seam and for wire major **4**
- On-card contract: [`FLAT_Store_Format.md`](FLAT_Store_Format.md)
- Replaces: the Device Object System v2 wire, system and registry contracts

Two contracts, one document, because they are two sides of the same narrow boundary. §2 is the seam
between the store and everything above it inside the firmware. §3 is what crosses the link to a
phone, a cable client, or the simulator. §4 binds §3 to BLE and USB.

Integers are unsigned little-endian; signed fields say so and are two's complement at their stated
width. Reserved bytes are zero and rejected when nonzero. Every message in this document is a fixed
layout with no extension block: a frame carrying a byte past the end of its stated layout is a
framing error, exactly as a short one is. `MUST`, `MUST NOT`, `SHOULD` and `MAY` have their RFC 2119
meanings.

CRC is the one in [`obc-crc`](../firmware/obc-crc) — CRC-32/IEEE, `crc32("123456789") = 0xCBF43926` —
and covers whole object payloads, never frames. Frame integrity belongs to the link (BLE Link Layer
CRC and retransmission, USB packet CRC and retry).

`ObjectId`, `Revision`, `StoreId`, object kinds and entry flags are defined by
[`FLAT_Store_Format.md`](FLAT_Store_Format.md) §3 and carry the same values here.

## 1. What the design refuses

Stated once, because most of what follows is short for these reasons and does not repeat them.

- **One engine, one owner.** BLE and USB are byte-identical adapters over one transfer engine. One
  store owns the card. A storage change reaches neither.
- **One transfer at a time.** The device serves exactly one `PUT` or `GET`; a second is `busy`.
- **Nothing durable but a commit.** Everything short of it is atomically invisible and cancellable
  from either side.
- **No `OperationId`, no claim record, no result ring, no durable operation result.** The catalog is
  the result, and §3.4 is how a client reads it after a break. There is no `Unknown` to reconcile.
- **No resume, no checkpoints, no prefix-CRC exchange.** A broken transfer is discarded whole; the
  worst case is re-sending a whole map over USB, about twenty minutes, which is cheaper than the
  machinery resume needs. (Since OBCM v14 / #1420 that really is one object, so a late break costs
  the whole map rather than one shard of it — accepted, inside the same twenty minutes.)
- **No sessions.** The `RequestId` of the transfer's own request is the identifier.
- **No Hello, no capability discovery, no wire minor.** The major is a transport fact and every
  message fits every link.
- **No metadata envelopes, no schema registry, no draft parts.** An object is bytes, a kind, a name
  and a CRC — a map included, since OBCM v14 / #1420 made it one object with its terrain inside it
  (`OBCM_Spec.md` §1.3). The clause that used to stand here, "a map set is a manifest object naming
  shards by `ObjectId`", is retired with `OBCA_Spec.md` §5.
- **No fault frames on the stream channel.** A transfer has one outcome and it is the answer to its
  own request.

## 2. The store seam

Five object operations, and beside them the housekeeping a caller cannot do without: the two
releases, the catalog view and its honesty flag, the four facts a `LIST` page and an admission are
built from, and the ride's one exception. Nothing above this boundary names a block, an extent, an
LBA, a path or a filename; `ObjectId`, `Revision`, byte offsets and byte lengths are the whole
vocabulary.

```rust
/// The card, as everything above it sees it.
pub trait Store {
    type Handle;

    /// Reserve space for `bytes`. RAM state until a commit names it; released by `cancel` and by
    /// the next mount, which rebuilds the free map from the catalog and cannot see it.
    fn allocate(&self, bytes: u64) -> Result<Allocation, StoreError>;

    /// Append to an allocation. Writes are sequential and the total may not exceed the reservation.
    fn write(&self, allocation: &mut Allocation, bytes: &[u8]) -> Result<(), StoreError>;

    /// Apply `mutations` atomically and return the new catalog commit sequence. The one durable
    /// transition: it makes new bytes visible and old bytes free in the same instant.
    fn commit(&self, mutations: &[Mutation]) -> Result<u64, StoreError>;

    /// Resolve an object. `revision` of `None` takes the head; `Some(r)` takes exactly that
    /// revision, which is how a retained previous revision is reached.
    fn open(&self, id: ObjectId, revision: Option<Revision>) -> Result<Self::Handle, StoreError>;

    /// Random access inside an open object. Returns bytes read, short only at end of payload.
    fn read(&self, handle: &Self::Handle, offset: u64, buf: &mut [u8]) -> Result<usize, StoreError>;

    // Beside the five, and not object operations.

    /// Release a reservation without publishing it. **Mandatory** on every path that abandons a
    /// transfer — a cancel, a refusal, a validator rejection, a lost link — because dropping an
    /// `Allocation` releases nothing.
    fn cancel(&self, allocation: Allocation);

    /// Close an open object, releasing its hold. **Mandatory** for the same reason: dropping a
    /// `Handle` leaks its row and the extents it is holding until the next mount, so `open` and
    /// `close` are a pair.
    fn close(&self, handle: Self::Handle);

    /// Read-only catalog view. LIST, every menu, and the free-space answer come from here.
    /// It mutates nothing and names nothing below the seam, so it is not a sixth verb.
    fn entries(&self) -> impl Iterator<Item = EntryMeta> + '_;

    /// True when the last `entries` listing reached the end of the array. A listing that hit a
    /// media failure stops early with nowhere to report it, so anything that treats one as the
    /// catalog asks here first.
    fn entries_ok(&self) -> bool;

    /// Why this store refuses writes, if it does — the `readOnly` details of §3.9 come from here.
    fn mode(&self) -> Mode;

    /// The card's identity, which every `LIST` page carries.
    fn store_id(&self) -> StoreId;

    /// The catalog commit sequence: the staleness hint a paged listing is checked against.
    fn commit_sequence(&self) -> u64;

    /// The next `ObjectId` the cursor will hand out. **Reading it reserves nothing**: two creates
    /// that both read it before either commits name the same id, and the second commit is refused
    /// as a duplicate key. Safe only because §1 serves one transfer at a time, and safest read at
    /// the commit rather than held across one.
    fn next_object_id(&self) -> ObjectId;

    /// The ride exception, and the only way bytes become durable without a commit. Performs both
    /// halves of `FLAT_Store_Format.md` §7.2: flush whole 16 KiB payload pages from the tail into
    /// the recording entry's own extents, then write one journal slot.
    fn journal(&self, checkpoint: RideCheckpoint) -> Result<(), StoreError>;
}

pub enum Mutation {
    /// Publish a revision.
    Put { meta: EntryMeta, source: PutSource },
    /// Remove one entry. Its extents are free at the gate.
    Remove { id: ObjectId, revision: Revision },
}

pub enum PutSource {
    /// Publish the extents of a freshly written allocation, consuming it.
    Fresh(Allocation),
    /// Keep the extents the named entry already holds and change only its metadata —
    /// the flag, name, length and CRC edits that end a ride or retain a bundle.
    Amend,
}

pub struct RideCheckpoint<'a> {
    /// The entry carrying RECORDING; the store rejects a checkpoint naming anything else.
    pub id: ObjectId,
    pub revision: Revision,
    /// Payload bytes past the last flushed page, oldest first. The store flushes whole pages
    /// out of the front of this and journals whatever remains.
    pub tail: &'a [u8],
    /// CRC-32 of the whole ride payload through `flushed length + tail.len()`, carried by the
    /// caller across checkpoints and recovered from the journal after a cut.
    pub payload_crc: u32,
}

/// Why a mounted store refuses writes, or refuses everything (`FLAT_Store_Format.md` §5.6). The
/// `readOnly` details of §3.9 are chosen from it: the two exhausted cases still serve reads, and
/// `CardTooSmall` — the card is not the card the superblock describes — reads out as `unformatted`.
pub enum Mode {
    ReadWrite,
    RevisionSpaceExhausted,
    SequenceSpaceExhausted,
    CatalogUnreadable,
    Unformatted,
    CardTooSmall,
}

pub enum StoreError {
    NotFound,
    RevisionConflict { current: Revision },
    NoSpace { required: u64 },
    TooFragmented,
    CatalogFull,
    Invalid,
    Media,
    ReadOnly,
}
```

The members below the comment separator sit beside the five and are counted separately because they
are not object operations. `cancel` and `close` are the releases the five have no way to express —
Rust's `Drop` cannot reach the store, so an abandoned reservation or a dropped handle would otherwise
hold its row until the next mount. `entries` mutates nothing and names nothing below the seam, and
`entries_ok` is how its one failure mode reports itself. `mode`, `store_id`, `commit_sequence` and
`next_object_id` are facts, not verbs: they read resident state, touch no media, and a `LIST` page or
a `PUT` admission is built from them. `journal` is the ride exception the epic granted.

A binding may of course name them differently or fold the four facts into one; what is normative is
that each is reachable and that nothing else is.

**Every operation takes a shared reference to the store, the mutators included.** A store is shared,
not owned: a mounted map holds its object open for the life of the image while an upload
commits and a ride journals, and an exclusive write half makes that shape un-expressible rather than
merely awkward. (It was an open object *per shard* until OBCM v14 / #1420; one map is now one
handle, which is the accounting change FS7.5c lands.) An implementation carries whatever interior mutability that needs, under two rules.

1. **Granularity is per card command for the state a reader needs.** A 1,024-entry commit is ~36 write
   commands over ~250 ms; releasing between them lets a read interleave into the gaps, so the worst
   stall a reader sees is one command rather than a whole commit. Concretely: the catalog, the free
   map and whatever table records open objects must be unheld at every card command a write path
   issues, so that `open`, `read`, `entries`, `entries_ok` and the free-space answer are all
   serviceable throughout a `commit`.

   Writer-private state — a reservation's staging buffer, which no read operation names — may be held
   across the commands that drain it, because the alternative is copying that buffer per command and
   §1 serves one transfer at a time anyway. An implementation taking this carve-out states which state
   it covers and for how many commands. State must never be held across an `await`, across a callback,
   or across another seam call, carve-out or not.
2. **`close` on an object another reader still holds is a runtime refusal, not a teardown.** The
   reader refcount §6.2 already requires is what decides it: such a `close` spends a count and returns,
   and the extents come back only when the last reader lets go. What the exclusive write half used to
   guarantee at compile time is therefore still guaranteed — *refused*, never *silent*.

The `&mut` on `write`'s allocation is the caller's own token and stays: an `Allocation` carries a
cursor that has to advance with the store's row. Mount and initialization are constructors and are
exempt — nothing else can reach the store while one runs.

`EntryMeta` is the metadata half of a catalog entry — kind, flags, `ObjectId`, `Revision`, payload
length, payload CRC, display name — and nothing else. `Allocation` is opaque: it exposes its reserved
length. **Neither carries an extent**, which is what makes the sentence at the head of this section
true rather than aspirational: extents enter the store through `allocate`, leave it never, and the
only thing a caller ever holds is the opaque token that stands for them.

`journal` is the one place where the "nothing above names an extent" rule needs a written reason
rather than a definition: flushing a payload page is a write into an object's own extents that no
commit certifies, so it cannot be `write` (which appends to an uncommitted allocation) and it cannot
be `commit`. It is the exception the epic granted the ride, and it is deliberately the only one.

Mount and initialization are lifecycle, not seam: they are constructors, and initialization is
destructive and explicit.

### 2.1 What a caller may rely on

- A `commit` that returns is durable. A `commit` that returns `Err` changed nothing.
- Bytes written to an `Allocation` that is never committed are unreachable and their space is free
  again at the next mount, or immediately on `cancel`. There is no cleanup step and nothing to sweep.
- An open `Handle` keeps reading the revision it resolved, even across a commit that replaces or
  removes it, until it is **closed**. This hold is RAM-only. Dropping the handle instead of closing
  it releases nothing: the row and its extents stay taken until the next mount, and the same is true
  of a dropped `Allocation`. `close` and `cancel` are the only ways back, and every abandonment path
  owes one of them.
- `read` is arithmetic on the entry's ranges: cost is one media read, with no chain walk and no
  indirection block. A reader that needs many small reads pays for the media, not for the format.
- A `journal` that returns `Ok` has flushed exactly `tail.len() / 16_384` whole pages, and the caller
  may drop that prefix from its own tail. A `journal` that returns `Err` has flushed **none** of it: the
  caller keeps the whole tail and retries with the same bytes, or with a tail that extends them. The
  flushed length moves only when the store says it did.
- A `write` that returns `Err` has advanced the allocation by nothing: the caller may retry the same
  bytes — a fragmented allocation is several media writes, and the ones that landed are the bytes the
  retry writes there again — or abandon the transfer, and a `cancel` of an allocation a `write` failed on
  always releases its row and its extents.

## 3. Protocol v4

Wire major **4**. There is no negotiation: the major is a transport fact readable before any frame
(§4), the frame ceiling is a property of the link, and every message below fits inside the smallest
ceiling either link offers. There is no Hello, no capability page, and no minor.

A client learns the store's identity and the freshness of its cache from `LIST`, which every client
issues before it does anything else. A `StoreId` it has not seen means the card was re-initialized
and everything it cached is void.

### 3.1 Control frame

Every control record carries exactly one control frame with this 16-byte header:

| Offset | Size | Field | Rule |
| --: | --: | :-- | :-- |
| 0 | 4 | magic | ASCII `OBC4` (`4F 42 43 34`) |
| 4 | 1 | wire major | `4` |
| 5 | 1 | opcode | §3.2 |
| 6 | 2 | flags | response bit 0, error bit 1, more bit 2; bits `3..15` zero |
| 8 | 2 | payload length | exact bytes after this header |
| 10 | 2 | reserved | zero |
| 12 | 4 | `RequestId` | nonzero; a response echoes its request |

Requests carry no flags. A successful response sets `response`; an error sets `response|error` and
its payload is exactly one 16-byte `ErrorBody` (§3.9). `more` is valid only on a `LIST` response.
There are no unsolicited control frames: everything the device needs to say is the answer to
something.

A `RequestId` is chosen by the client and is not reused while its request is outstanding. A zero
`RequestId` is unanswerable — a response would have to echo it — so a receiver emits nothing and
closes that record stream.

**`RequestId` is also the transfer identifier.** A `PUT` or `GET` is one long-running request: the
device answers it exactly once, when the transfer terminates, and the stream frames that belong to it
carry the same value (§3.8). That is why there is no session identifier in this protocol.

### 3.2 Opcodes

| Opcode | Operation | Durable effect |
| --: | :-- | :-- |
| `0x01` | `LIST` | none |
| `0x02` | `STATUS` | none |
| `0x03` | `GET` | none |
| `0x04` | `PUT` | one commit, on success only |
| `0x05` | `REMOVE` | one commit |
| `0x06` | `CANCEL` | none |
| `0x07` | `ARM` | one commit, then the boot handoff and a reboot |

An unknown opcode is `unsupported`. There is no generic forwarding path.

### 3.3 `LIST`

Request, 32 bytes:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 2 | kind filter; `0` lists every kind |
| 2 | 2 | flags: cursor bit 0; other bits zero |
| 4 | 4 | zero |
| 8 | 8 | cursor `ObjectId`; zero unless the cursor bit is set |
| 16 | 8 | cursor `Revision`; zero unless the cursor bit is set |
| 24 | 8 | expected commit sequence; zero unless the cursor bit is set |

The cursor is the **pair**, and the page resumes strictly after it, because the catalog is keyed by
`(ObjectId, Revision)` and an object may hold two entries. A cursor of `ObjectId` alone would skip the
head of an object whose retained revision ended the previous page — the retained entry sorts first
(`FLAT_Store_Format.md` §5.3), so that page boundary is not exotic: it is two entries wide on BLE and
would silently drop the current revision of the very object a client asked about.

Response payload is a 24-byte prefix followed by `n` entries:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 16 | `StoreId` |
| 16 | 8 | catalog commit sequence |

| Entry offset | Size | Field |
| --: | --: | :-- |
| 0 | 8 | `ObjectId` |
| 8 | 8 | `Revision` |
| 16 | 8 | payload length |
| 24 | 4 | payload CRC-32 |
| 28 | 2 | kind |
| 30 | 2 | flags |
| 32 | 1 | display-name length, `0..=48` |
| 33 | 3 | zero |
| 36 | 48 | display name, UTF-8, unused bytes zero |
| 84 | 4 | zero |

Entries are 88 bytes and arrive in the catalog's own `(ObjectId, Revision)` order, so the cursor for
the next page is the last entry's pair. The device sets `more` when a further page exists; the client
then repeats the request with the cursor bit, that pair, and the commit sequence it was told. A paged
request whose expected commit sequence no longer matches is `catalogChanged` with the current
sequence in the error body; the client restarts the listing. A first page never fails that way,
because it declares no expectation.

A retained revision appears as its own entry, flagged `RETAINED`. A client that does not care about
retention takes the greater `Revision` for an `ObjectId`.

### 3.4 `STATUS`

The reconcile path for everything that named an object. Request, 16 bytes: `ObjectId u64`,
`Revision u64`. Response, 24 bytes:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 1 | state: absent `0`, committed `1`, superseded `2` |
| 1 | 3 | zero |
| 4 | 8 | current head `Revision`; zero when absent |
| 12 | 8 | current head payload length; zero when absent |
| 20 | 4 | current head payload CRC-32; zero when absent |

`committed` means the catalog holds exactly the revision asked about as the head. `superseded` means
the object exists at a different revision. `absent` means no entry names that `ObjectId`.

A client whose link broke with a **replace** outstanding asks `STATUS` for the `ObjectId` and the
`Revision` it expected to produce. `committed` means the upload landed; anything else means it did
not, and re-sending is safe, because a replace that in fact landed fails its compare-and-swap the
second time.

A **create** cannot be reconciled this way, and the contract says so rather than implying otherwise:
the client sent `ObjectId` zero, the device assigned the id, and that assignment was in the response
that was lost. There is nothing to ask `STATUS` about. The client reconciles a lost create with
`LIST`, matching on `(kind, payload length, payload CRC, display name)` — the CRC is what makes that
match sound. Finding it means the create landed; not finding it means it did not. A false negative
costs one duplicate object, which the client removes with `REMOVE` once it sees both.

Closing that hole on the device would require exactly the durable claim record this design does not
have, so it is a client obligation, priced at one duplicate in a case that needs a link to break
inside the one round trip between commit and response.

A `STATUS` naming `ObjectId` zero is `invalidRequest`; the identity of the store comes from `LIST`.

### 3.5 `GET`

Request, 16 bytes: `ObjectId u64`, `Revision u64` (`0` = current head).

The device resolves and opens the object, then streams its payload on the stream channel in ascending
contiguous offsets under the request's `RequestId`. When the last byte has been handed to the
transport it answers the request, 24 bytes:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 8 | `Revision` served |
| 8 | 8 | payload length |
| 16 | 4 | payload CRC-32 |
| 20 | 4 | zero |

The client verifies length and CRC itself. A refusal — `notFound`, `busy`, `mediaIo` — is an error
response to the same `RequestId`, and it may arrive before, during or instead of the stream; a client
that sees it discards whatever it has.

A `GET` on an entry carrying `RESERVED` or `RECORDING` is `invalidRequest`, for the same reason `PUT`
and `REMOVE` refuse them: the store did not write the reserve's bytes, and a recording ride's length
and CRC are zero until the commit that ends it, so serving one would report success over an empty
payload. A client syncs a ride once `RECORDING` has cleared from its `LIST` entry.

The open handle of §2.1 lives for the transfer, so a replace or a remove committed while a download
is running changes what is visible without disturbing the bytes being read.

### 3.6 `PUT`

Request, 84 bytes:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 8 | `ObjectId`; `0` creates a new object |
| 8 | 8 | expected `Revision`; zero when creating |
| 16 | 8 | declared payload length |
| 24 | 4 | declared payload CRC-32 |
| 28 | 2 | kind |
| 30 | 2 | flags: retain-previous bit 0; other bits zero |
| 32 | 1 | display-name length, `0..=48` |
| 33 | 3 | zero |
| 36 | 48 | display name, UTF-8, unused bytes zero |

The client streams the payload on the stream channel under this `RequestId`, from offset zero,
contiguous and ascending, ending at the declared length. It **may begin immediately**, without
waiting for an acceptance: the device has admitted the request and allocated by the time it processes
the first stream frame, and if it refuses it answers the request and discards frames bearing that
`RequestId`. The cost of a refusal is one round trip of wasted bytes, which is the price of not
having a second round trip on every upload.

That sentence rests on an ordering the two channels do not provide by themselves — on BLE the control
write and the CoC are independent — so §5 makes it an **adapter obligation**: a control frame reaches
the engine before any stream frame bearing the same `RequestId`. Without it the first frame arrives
pre-admission, is discarded as belonging to no live transfer, and the upload dies on a gap at offset
zero.

On the last byte the device verifies the length and the whole-payload CRC, runs the kind's validator,
and commits. The response, 32 bytes:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 8 | `ObjectId`, assigned when the request created one |
| 8 | 8 | new `Revision` |
| 16 | 8 | committed payload length |
| 24 | 4 | committed payload CRC-32 |
| 28 | 4 | zero |

The expected `Revision` is checked at admission and again immediately before the commit. `0` in
`ObjectId` means create and both fields must be zero; a nonzero `ObjectId` means replace and the
expected `Revision` must be the value the device last reported for it. Zero is not a wildcard in
either field.

A `PUT` naming an entry that carries `RECORDING` or `RESERVED` is refused `invalidRequest`, and a
`PUT` of kind `3` (ride) or `8` (rollback reserve) is refused the same way whether it creates or
replaces: those two kinds are produced by the device, and a client that could overwrite a ride
mid-recording or a rollback reserve mid-update would be writing where the store and the bootloader
already are.

The request's flag word says what this upload should do, not what the resulting entry carries:
`RECORDING`, `RETAINED` and `RESERVED` are the format contract's entry flags, they appear in a `LIST`
entry, and no client sets them. `retain-previous` asks the same commit to leave the displaced revision
`RETAINED` (`FLAT_Store_Format.md` §5.3); it is legal only for kinds whose reader needs continuity —
weather, today — and a second retaining replace frees the first.

**A replace leaves at most what it asked for.** One commit publishes the new head, retains or removes
the revision it displaced, and frees any revision the object was already keeping retained — so a
replace *without* the flag clears retention outright, and never leaves a revision two generations back
alive behind a head that did not ask for it. That matters to a client because retention is durable and
visible: `LIST` shows the retained entry, `GET` can pin it, and `REMOVE` takes it with the head. A
client that wants continuity sets the flag on every replace of that object.

**Any break before the commit leaves the card as if nothing happened**: the allocation is released,
the written bytes are anonymous, the catalog is untouched, and the client restarts from zero. That
holds for a cable pull, a cancel, a CRC failure, a validator refusal, and a power cut alike.

### 3.7 `REMOVE`

Request, 16 bytes: `ObjectId u64`, expected `Revision u64`. One commit removes the entry and frees
its extents; a retained previous revision of the same object goes with it. Response, 8 bytes: the new
catalog commit sequence.

A `REMOVE` of an entry carrying `RECORDING` or `RESERVED` is `invalidRequest`. Stopping a ride and
settling an armed update are device-local acts, not wire ones, and freeing either object's extents
under the store or the bootloader is exactly what those flags exist to prevent.

### 3.8 `CANCEL` and the stream channel

Every stream record carries one 16-byte frame and no payload checksum:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | `RequestId` of the `PUT` or `GET` this belongs to |
| 4 | 8 | absolute payload offset |
| 12 | 2 | payload length, nonzero |
| 14 | 2 | zero |

A stream record is this 16-byte frame immediately followed by exactly `payload length` payload bytes,
and one record is one link record: one CoC SDU on BLE, one length-prefixed record on USB. The frame
carries no direction byte because it needs none — the `RequestId` names a `PUT` or a `GET`, and that
settles which way the bytes go.

Frames are contiguous and ascending; the offset equals the receiver's next expected offset. A gap, an
overlap, a zero length, a length disagreeing with the record, or a length above the link's ceiling
terminates the transfer with an error response on the control channel. There are no fault frames, no
terminal flags and no acknowledgements on this channel: the transfer's one outcome is the answer to
its control request.

A frame bearing a `RequestId` that is not the live transfer's is discarded in silence. Late frames
from a transfer the peer has already been told about are ordinary in-flight traffic, not an attack. A
frame bearing the `RequestId` of a live **`GET`** is discarded the same way: that identifier already
settles which way the bytes go, so bytes arriving against the direction it names belong to no transfer
the receiver can be sure of.

That silence is also why a client **SHOULD NOT** reuse a `RequestId` immediately after the answer to
the request that carried it. A `PUT` that was terminated mid-stream can leave in-flight stream frames
on the link; a new transfer that reuses the identifier absorbs them as its own, and a stale offset
kills it with `invalidRequest`/`streamOffset`. Identifiers are 32 bits and cost nothing — advancing is
the whole remedy.

**Cancel is bilateral and symmetric.** The client cancels with `CANCEL`, request 4 bytes
(`RequestId u32`), response 1 byte: `0` cancelled, `1` no such transfer. The cancelled `PUT` or `GET`
also receives its own error response, `cancelled`. The device cancels by answering the outstanding
`PUT` or `GET` with an error and dropping the transfer. Either way the allocation is released and the
catalog is unchanged; there is nothing else to unwind.

Link teardown is the third form of the same thing: the adapter calls the engine once, the live
transfer is dropped, the allocation is released, and no record of it exists.

### 3.9 Errors

An error response payload is exactly this 16-byte body:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 2 | code |
| 2 | 2 | detail, code-scoped; `0` means no narrower fact |
| 4 | 8 | context, code-scoped; zero when the code defines none |
| 12 | 4 | zero |

There is no diagnostic text. Text drove nothing in the contract this replaces, and a device with 16
bytes to spare per error has better uses for the frame.

| Code | Name | Context | Details |
| --: | :-- | :-- | :-- |
| 1 | `unsupported` | — | opcode `1`, kind `2`, wire major `3` |
| 2 | `invalidFrame` | — | magic `1`, length `2`, truncated `3`, trailing `4` |
| 3 | `invalidRequest` | — | reservedBits `1`, unknownEnum `2`, badCombination `3`, streamOffset `4` |
| 4 | `notFound` | — | object `1`, revision `2` |
| 5 | `revisionConflict` | current head `Revision` | headDiffers `1`, headAbsent `2` |
| 6 | `noSpace` | bytes required | extents `1`, catalogFull `2`, tooFragmented `3` |
| 7 | `checksumFailure` | declared payload CRC | payload `1` |
| 8 | `mediaIo` | — | read `1`, write `2`, sync `3` |
| 9 | `busy` | `RequestId` of the live transfer | transfer `1` |
| 10 | `cancelled` | — | byClient `1`, byDevice `2`, linkLost `3` |
| 11 | `rejected` | kind-specific | the kind's validator owns the detail space |
| 12 | `internal` | — | — |
| 13 | `catalogChanged` | current commit sequence | listing `1` |
| 14 | `readOnly` | — | catalogUnreadable `1`, revisionSpaceExhausted `2`, unformatted `3` |

Code `0` is invalid and is treated as a malformed body. A receiver reads a code it does not know as a
failure it cannot classify; it never treats an unknown code as success.

`readOnly` is the wire face of a store that cannot be written: no usable catalog, an exhausted
revision space, or a card §5.6 step 1 classified as **not a flat store**
(`FLAT_Store_Format.md` §3, §5.6). Every opcode returns it in the unformatted and unreadable cases,
including the reads, because there is nothing to read; only the exhausted case still serves reads.
Initializing such a card is destructive and device-local — no wire opcode formats a card.

Two of the store's read-only modes share one detail and one card too small shares another, so the
mapping is written out rather than inferred:

| `Mode` (§2) | detail |
| :-- | :-- |
| `CatalogUnreadable` | `catalogUnreadable 1` |
| `RevisionSpaceExhausted` | `revisionSpaceExhausted 2` |
| `SequenceSpaceExhausted` | `revisionSpaceExhausted 2` — no third value is registered, and from a client's side both are this card's counters running out |
| `Unformatted` | `unformatted 3` |
| `CardTooSmall` | `unformatted 3` — the card the superblock describes is not the card in the slot, so there is no flat store here either |

An error means the mutation did not happen, with exactly one exception a client must handle: a
response lost after the commit. That is what §3.4 is for, and it is why no error code claims an
uncertain outcome.

### 3.10 Vectors

`StoreId` and the two objects are those of [`FLAT_Store_Format.md`](FLAT_Store_Format.md) §5.7.

`PUT` creating the route, `RequestId 0x00002A01`, 100 bytes on the wire:

```
0000  4F 42 43 34 04 04 00 00 54 00 00 00 01 2A 00 00
0010  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0020  99 A4 00 00 00 00 00 00 21 7E 4A 9C 01 00 00 00
0030  0C 00 00 00 47 72 69 6D 73 65 6C 20 4C 6F 6F 70
0040  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0050  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0060  00 00 00 00
```

A stream frame of that upload: offset 40,960, 1024 payload bytes, followed on the wire by those 1024
bytes.

```
0000  01 2A 00 00 00 A0 00 00 00 00 00 00 00 04 00 00
```

The error response if the route already exists at another revision — `revisionConflict`, detail
`headDiffers`, current head `Revision 5`:

```
0000  4F 42 43 34 04 04 03 00 10 00 00 00 01 2A 00 00
0010  05 00 01 00 05 00 00 00 00 00 00 00 00 00 00 00
```

A complete `LIST` response, `RequestId 0x00002A02`, both entries, no further page, 216 bytes:

```
0000  4F 42 43 34 04 01 01 00 C8 00 00 00 02 2A 00 00
0010  8F 2C 41 D9 6B 07 4E A3 B1 55 9C 20 7D E8 34 66
0020  07 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00
0030  03 00 00 00 00 00 00 00 99 A4 00 00 00 00 00 00
0040  21 7E 4A 9C 01 00 00 00 0C 00 00 00 47 72 69 6D
0050  73 65 6C 20 4C 6F 6F 70 00 00 00 00 00 00 00 00
0060  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0070  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0080  02 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00
0090  00 00 00 00 00 00 00 00 00 00 00 00 03 00 01 00
00a0  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00b0  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00c0  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00d0  00 00 00 00 00 00 00 00
```

## 4. Firmware update

An update package arrives as an ordinary `PUT` of kind `7`. Uploading never installs. `ARM` is the
one explicit step that makes an installed image the next boot, and it is a separate authenticated
command precisely so that delivery and installation are different decisions.

Request, 16 bytes: package `ObjectId u64`, expected `Revision u64`. The device then, in this order:

1. **Validates the pinned package**: `obc-dfu` checks the OBCU structure, the image CRC, the Ed25519
   signature, and version monotonicity against the running image. It refuses a package that is not
   strictly newer. It also refuses while a ride is recording or the battery is below the install
   threshold. Every one of those refusals is `rejected` with the update kind's detail — the
   ride-recording one included, since it is a fact about whether this package may install now and not
   about a transfer holding the engine — and changes nothing.
2. **Allocates and commits the rollback reserve**: one entry of kind `8` with the `RESERVED` flag and
   enough extents for the running image. This is the one commit `ARM` makes, and it exists because
   the bootloader cannot allocate — it can only write where it is told.
3. **Writes the boot handoff**: both extent lists, resolved to absolute 512-byte block runs, into the
   RRAM boot-state page, then reads it back and verifies. The page format and the bootloader's
   decision logic are [`OBCU_Spec.md`](OBCU_Spec.md); its `Extent { start_block, blocks }` is exactly
   what §6.1 of the format contract computes, and its 96-extent capacity is twelve times the eight
   ranges an object can have. **The bootloader's shape does not change.**
4. **Answers and drains**: the response, 16 bytes — rollback `ObjectId u64`, new catalog commit
   sequence `u64` — must reach the transport before the reboot, or the adapter's bounded drain
   timeout must expire.
5. **Reboots.**

Step 3 is also the one place an `ARM` can fail with a commit already made. A refusal there — the page
did not read back — is **not** the cut below: a cut reboots, and the reboot is what runs the
reconciliation. So the device takes step 2's commit back and answers `internal`, and §3.9's rule that
an error means the mutation did not happen holds. If that removal is refused too, the device answers
the error and stops: what it leaves is exactly the state a cut leaves — a boot page that does not
decode and an orphaned reserve — which the next boot's reconciliation settles with one commit. It
self-heals; it does not accumulate.

`ARM` is not cancellable once it has been answered. A cut anywhere before step 3 completes leaves a
boot page that does not decode, which the bootloader reads as *no pending update*; the rollback
reserve is then an ordinary object the next boot removes. A cut after step 3 is the bootloader's
business, and its trial boot and rollback are unchanged by this document.

The rollback reserve is the single deliberate exception to the format's rule that the store writes an
object's bytes: the bootloader writes into those extents. The entry exists to keep them out of the
allocator, nothing more, and post-install reconciliation removes it with one commit.

There is no card-sideload path. The card is not user-accessible and the only delivery routes are
BLE, USB and the builder.

## 5. Transport bindings

The frame bytes of §3 are identical on both links. An adapter owns record boundaries, pacing,
timeouts, drain, and nothing else. It never parses a payload, never mints an identifier, and never
originates a frame.

**Version before framing.** Neither peer can negotiate a major it cannot frame, so each binding
exposes the major as a transport fact readable before the first frame. A peer that reads a major it
does not implement takes its own mismatch path and never sends a frame it would have to misparse.

**Cross-channel ordering.** An adapter MUST deliver a control frame to the engine before any stream
frame bearing the same `RequestId`. The two channels are independent transports — an ATT write and an
L2CAP CoC, two USB endpoint pairs — and neither orders itself against the other, so this is the one
guarantee §3.6 needs the binding to supply and the reason a `PUT` may stream without an acceptance.
An adapter that cannot order the two MUST hold one link-ceiling stream frame for a `RequestId` it has
not yet seen admitted; it MUST NOT deliver it, and it MUST NOT drop it. While it holds that frame it
MUST withhold link credit — CoC credits on BLE, ceasing to accept stream records on USB — rather than
buffer a second one. §3.6 lets a client stream as fast as the link allows, so backpressure is the only
thing that bounds this: without it the second pre-admission frame has nowhere to go and no defined
disposition.

### 5.1 BLE

The device serves the OBC Control service and advertises its 128-bit UUID; discovery, the stable
static random address, bonding and reconnect are unchanged.

| UUID | Name | Properties | Role |
| :-- | :-- | :-- | :-- |
| `3C920000-9916-4EBA-ABC2-342FE08F6B10` | OBC Control service | — | the advertised service |
| `3C920008-9916-4EBA-ABC2-342FE08F6B10` | `protocolVersion` | read, open | two bytes, `u16` = `4` |
| `3C920007-9916-4EBA-ABC2-342FE08F6B10` | `psm` | read, open | `u16` dynamic L2CAP PSM of the stream channel |
| `3C920009-9916-4EBA-ABC2-342FE08F6B10` | `objectControl` | Write Request + Indicate, authenticated | the control channel |

- One `objectControl` Write Request value carries one complete control frame; one confirmed
  indication carries its response. A client enables indications before its first write.
- One L2CAP CoC SDU on the advertised PSM carries one complete stream frame. A frame never spans
  SDUs and an SDU never carries two.
- The control ceiling is `ATT_MTU - 3`; the device's preferred 247-byte MTU gives 244 bytes. The
  largest fixed message in §3 is the 100-byte `PUT`, and a `LIST` page carries as many 88-byte
  entries as the ceiling allows — two on BLE, tens on USB — so the protocol floor is the 128 bytes a
  single-entry page needs. A link below that floor cannot carry this protocol and the adapter refuses
  the connection rather than truncating. The stream ceiling is the CoC SDU, fixed at channel
  establishment.
- CoC credits are pacing. They acknowledge nothing about durability.
- `3C920009` keeps its v3 meaning across this major bump rather than being retired and reassigned.
  The no-reuse convention of [`obc-ble-interface-spec.md`](obc-ble-interface-spec.md) forbids giving a
  retired UUID a *new* meaning; this characteristic keeps the one it has — the control channel — and
  what changed is the frames it carries, which `protocolVersion` already announces.
- Before an `ARM` reboot, the terminal indication must be confirmed and accepted outbound records
  must complete, or the drain timeout must expire.

### 5.2 USB

The vendor interface keeps `bInterfaceClass = 0xFF`, `bInterfaceSubClass = 0x00`, and reports
`bInterfaceProtocol = 4`; the device descriptor's `bcdDevice` carries the major in its high byte,
`0x0400`. Matching therefore settles the version before a record is exchanged. Enumeration is the
authorization boundary on this link: physical possession of the port is the trust decision, and the
package signature and version check are what bound what may run on the device.

- One control bulk endpoint pair and one stream bulk endpoint pair. Each record is `record_length
  u16` followed by exactly that many frame bytes. Packet boundaries carry no protocol meaning; a
  record may span packets, but records are neither interleaved nor concatenated without their
  prefixes.
- A zero, out-of-range, truncated or overrun record length is `invalidFrame` and resets that record
  stream before teardown is reported to the engine.
- Before an `ARM` reboot, the response record and every earlier IN record must complete at the
  device-controller layer, or the drain timeout must expire.

There is no USB mass storage binding and there will not be one: it would hand the host raw blocks and
force the firmware off the card.
