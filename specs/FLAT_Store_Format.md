# Flat store on-card format

- Status: **normative** for the flat card store (Device Object System v3, epic #1256)
- Format version: 1
- Seam and wire contract: [`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md)
- Replaces: every earlier on-card layout, catalog, staging, promotion and sidecar mechanism

This document is the byte contract for the raw SD card. There is no partition table and no
filesystem: the card is five fixed regions and an array of 1 MiB extents, and every address the
store computes is arithmetic on those constants.

The rule the whole format serves:

> An object never changes. New bytes get new space. One commit makes the new bytes visible and
> makes the old bytes free.

The active ride is the one exception — it grows for hours — and §7 is the one mechanism it gets.

Integers are unsigned little-endian unless a field says signed, in which case it is two's complement
at the stated width. Byte offsets are zero-based. Reserved **fields inside a record** are written as
zero and MUST be zero when read. Space **outside** any record — the region tail of §2, the superblock
pad of §4, the catalog tails of §5.1 — is whatever an earlier commit or an earlier card left there:
nothing reads it, no CRC covers it, and initialization does not spend writes zeroing it.
`MUST`, `MUST NOT`, `SHOULD` and `MAY` have their RFC 2119 meanings. A decoder
rejects an unknown magic, an unknown format version, a count above its stated capacity, a range that
leaves the extent area, or arithmetic overflow **before** it uses any derived address.

CRC fields are CRC-32/IEEE: reflected polynomial `0xEDB88320`, initial value and xor-out
`0xFFFFFFFF`, check value `crc32("123456789") = 0xCBF43926`. The one implementation is
[`obc-crc`](../firmware/obc-crc); no other polynomial or parameterisation appears in this format. A
CRC field reads as zero while the bytes it covers are checksummed. CRC detects accidental
corruption; it is not identity, authentication, or an idempotency proof.

## 1. Media fault model

A 512-byte block write is **not** assumed to be all-or-nothing. A power cut during programming may
corrupt any block inside the media **program page** being programmed. The program page `P` is a
format constant of **16,384 bytes**, and every region boundary and record stride below is a multiple
of it, measured from physical LBA 0. A write may corrupt blocks inside the page it is programming
and does not corrupt blocks lying in another page.

That assumption is **taken, not yet measured**: the rig that removes the card's supply rail mid-write
is #1383, deferred for want of board access. If it later fails, the remedy is region spacing and copy
counts inside this document and inside the store — nothing above the seam of
[`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §2 changes.

Two consequences are used throughout and stated once here:

- A record written **in one shot and covered by one CRC over all of its pages** is all or nothing to
  a reader: a cut corrupts one of those pages, the CRC fails, and the record is skipped. It does not
  matter how many pages the record spans, only that no reader ever sees a subset of the write. This
  is why ride journal slots — two pages, one write, one CRC — need no gate of their own (§7).
- A record too large to write in one shot needs a **gate**: a separate, later, single-block write
  that certifies the body. The gate sits in a page of its own so that programming it can never
  damage the body it certifies. This is the catalog (§5).

## 2. Card geometry

The store owns the whole card from LBA 0. Blocks are 512 bytes. The first 2 MiB is the fixed region;
the extent area is everything after it.

| LBA range | Blocks | Size | Region |
| :-- | --: | --: | :-- |
| `0 .. 32` | 32 | 16 KiB | superblock copy A (§4) |
| `32 .. 64` | 32 | 16 KiB | superblock copy B (§4) |
| `64 .. 576` | 512 | 256 KiB | catalog copy A (§5) |
| `576 .. 1088` | 512 | 256 KiB | catalog copy B (§5) |
| `1088 .. 2112` | 1024 | 512 KiB | ride journal, 16 slots × 32 KiB (§7) |
| `2112 .. 4096` | 1984 | 992 KiB | reserved; never written and never read |
| `4096 .. ` | — | rest of card | extent area, 1 MiB extents (§6) |

Every one of those boundaries is a multiple of 16,384 bytes, so no two regions share a program page
and the reserved tail places the extent area on a 1 MiB boundary, which is also what makes §6's
address arithmetic exact.

**Block 0 is deliberately not an MBR.** Its bytes `510..511` are zero (§4 puts the superblock CRC at
`504..508` for exactly this reason), so a host that inspects the card sees an unformatted device
rather than a partition table it might try to repair.

## 3. Identity

Three identities, and no others. `GenerationId` and `OperationId` do not exist in this format:
uncommitted bytes are anonymous, and the catalog is the only durable record of a result.

| Identity | Width | Scope |
| :-- | --: | :-- |
| `StoreId` | 16 bytes | the card. Drawn from the device CSPRNG at initialization; never changes; a new one means everything a client cached is void. |
| `ObjectId` | `u64` | store-global, allocated from the catalog header's monotonic cursor, **never reused**. `0` is reserved and names no object. |
| `Revision` | `u64` | per object. `1` for the commit that creates it, `+1` for every commit that replaces it. Never wraps; reaching `u64::MAX` mounts the store read-only. |

An object is identified by `ObjectId` alone. `Revision` is the compare-and-swap token every mutation
carries and the discriminator between the two entries an object may have while a previous revision is
retained (§5.3).

### 3.1 Object kinds

`kind` is a `u16`. This section is the sole authority for the values; the wire contract carries the
same numbers and defines no others.

| Value | Kind | Notes |
| --: | :-- | :-- |
| 0 | invalid | never encoded |
| 1 | route | OBCR payload |
| 2 | trip | ordered route membership |
| 3 | ride | produced by the device; the one growing object (§7) |
| 4 | weather bundle | OBCW payload; at most one retained previous revision |
| 5 | map shard | OBCM or OBCT payload |
| 6 | map set manifest | names shards by `ObjectId`; a set activates when its manifest commits |
| 7 | update package | OBCU image |
| 8 | firmware rollback reserve | extents owned by the store, payload written by the bootloader (§5.3) |

## 4. Superblock

Two copies, A and B, of identical bytes. The superblock is written **once**, by initialization, and
never again — nothing in normal operation updates it, which is why it carries no gate and no
sequence. Two copies exist so that one bad block does not make the card unreadable; a mount takes
copy A when it validates and copy B otherwise.

The body is block 0 of the copy. Blocks `1..32` of the copy are outside the record: never written,
never read.

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `FSSB` (`46 53 53 42`) |
| 4 | 2 | format version, `1` |
| 6 | 2 | zero |
| 8 | 16 | `StoreId` |
| 24 | 8 | total card blocks observed at initialization |
| 32 | 472 | zero |
| 504 | 4 | CRC-32 over bytes `0..504` |
| 508 | 4 | zero |

A superblock is valid when its magic, version and CRC all check. Nothing else is stored, because
nothing else varies: the region layout of §2 is a constant of format version 1, and the extent count
is a function of the block count that §6 recomputes at every mount. A card whose layout differs is a
different format version, which the version field already names.

If a mount observes a card larger than `total card blocks`, the surplus is unused; a card smaller
than that value is refused as damaged or swapped, never silently truncated.

### 4.1 Vector

`StoreId = 8F2C41D96B074EA3B1559C207DE83466`, a 32 GB card — 62,914,560 blocks, 30 GiB — from which
§6 recomputes 30,718 extents.

```
0000  46 53 53 42 01 00 00 00 8F 2C 41 D9 6B 07 4E A3
0010  B1 55 9C 20 7D E8 34 66 00 00 C0 03 00 00 00 00
0020  ..                                        (zero to 504)
01F8  CC 5C 51 1D 00 00 00 00
```

CRC-32 over bytes `0..504` is `0x1D515CCC`.

## 5. Catalog

The catalog **is** the store. It names every object that exists, which extents each one occupies, and
nothing else. The free-extent bitmap is its complement, recomputed at mount; there is no free list on
the card and nothing to reconcile.

Two copies alternate. One commit writes the inactive copy and then its gate; the gate is the moment
the new bytes become the truth.

### 5.1 Copy layout

Each copy is 256 KiB = 512 blocks = 16 program pages.

| Blocks (copy-relative) | Bytes | Content |
| :-- | :-- | :-- |
| `0` | `0 .. 512` | catalog header (§5.2) |
| `1 .. 480` | `512 .. 245760` | entry array, 1916 × 128 bytes (§5.3) |
| `480` | `245760 .. 246272` | gate sector (§5.4) |
| `481 .. 512` | `246272 .. 262144` | outside the record: never written, never read |

The **body** is the header followed by exactly `entry count` entries — `512 + entry_count × 128`
bytes — and nothing else. Bytes of the entry array beyond the live prefix are whatever an earlier
commit left there; they are never read and are not covered by any CRC. That is what makes the
used-entries copy of §5.5 possible: a commit writes `1 + ceil(entry_count / 4)` blocks, not 480.

The gate occupies a program page of its own (blocks `480..512`), so programming the gate cannot
damage the last entries of the body it certifies.

Entry capacity is `479 × 4 = 1916`, and `512 + 1916 × 128 = 245,760` bytes fills blocks `0..480`
exactly.

### 5.2 Header

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `FSCT` (`46 53 43 54`) |
| 4 | 2 | format version, `1` |
| 6 | 2 | entry stride, `128` |
| 8 | 16 | `StoreId` |
| 24 | 8 | commit sequence |
| 32 | 8 | next `ObjectId` to allocate |
| 40 | 2 | entry count, `0..=1916` |
| 42 | 470 | zero |

The commit sequence starts at `1` at initialization and increments by exactly one per commit. It is
store-global, never wraps, and is the value a client uses to tell whether its cached listing is
stale. `next ObjectId` is strictly greater than every `ObjectId` in the array and is never rewound;
an object removed does not return its id.

The header carries no CRC of its own — it is part of the body, and the gate is what certifies the
body.

### 5.3 Entry

Exactly 128 bytes. One entry names one revision of one object.

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 2 | kind (§3.1), nonzero |
| 2 | 2 | flags (below) |
| 4 | 1 | range count, `1..=8` |
| 5 | 1 | display-name length, `0..=48` |
| 6 | 2 | zero |
| 8 | 8 | `ObjectId`, nonzero |
| 16 | 8 | `Revision`, nonzero |
| 24 | 8 | payload length in bytes |
| 32 | 4 | payload CRC-32 over the whole payload |
| 36 | 4 | zero |
| 40 | 32 | 8 × extent range: `u16 first extent`, `u16 extent count` |
| 72 | 48 | display name, UTF-8, unused bytes zero |
| 120 | 8 | zero |

Ranges `range count .. 8` are all zero. Each live range has a nonzero extent count, lies wholly
inside the extent area, and does not overlap any other range of any entry — the mount that builds
the free bitmap (§5.6) checks exactly that and fails the copy if it does not hold. Ranges are in
payload order: range `i` carries the payload bytes that follow range `i-1`.

The ranges MUST cover at least `ceil(payload length / 1 MiB)` extents. They may cover more only while
the `RECORDING` or `RESERVED` flag is set; every other entry is trimmed to its payload at the commit
that publishes it, so the tail of the last extent is the only slack an ordinary object carries.

Flags:

| Bit | Name | Meaning |
| --: | :-- | :-- |
| 0 | `RECORDING` | the active ride. Payload length and CRC are the values of the last commit, not of the current recording; the ride journal (§7) is authoritative for what is beyond them. At most one entry in the catalog carries it. |
| 1 | `RETAINED` | this entry is a non-head revision the store keeps alive on purpose, so that a reader mid-stream and a domain that wants continuity — weather's previous bundle, today — still have bytes. It is set and cleared only by a commit, and everything else in this suite refers here rather than restating it. |
| 2 | `RESERVED` | the entry owns extents and the store does not write the payload. Only kind 8 uses it; the bootloader writes those bytes. Payload length is zero and `read` on it is refused. |

Bits `3..15` are zero.

**Display name** is what a menu shows. It is UTF-8, at most 48 bytes, and the store does not
normalise, trim or case-fold it. An empty name (length `0`) is legal — a ride has none until it is
finalised.

**Ordering and uniqueness.** Entries are sorted ascending by `(ObjectId, Revision)` and the pair is
unique. A lookup is therefore a binary search over the live prefix — at most nine block reads — and
the byte image of the catalog is a function of the store's state, which is what lets the FS3
reference model compare bytes rather than sets.

For one `ObjectId` the array holds either one entry, or exactly two of which precisely one carries
`RETAINED`. The entry without `RETAINED` is the **head** and has the greater `Revision`, so the
retained one sorts *before* it. Every entry of one `ObjectId` has the same kind. A copy that violates
any of this is structurally invalid.

### 5.4 Gate sector

The gate is the commit. It is one 512-byte block, written and synchronized after the body is
synchronized, and it is the only thing that makes the body it names authoritative.

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `FSCG` (`46 53 43 47`) |
| 4 | 2 | format version, `1` |
| 6 | 2 | copy index: `0` for A, `1` for B |
| 8 | 16 | `StoreId` |
| 24 | 8 | commit sequence, equal to the body's |
| 32 | 2 | entry count, equal to the body's |
| 34 | 2 | zero |
| 36 | 4 | body CRC-32 over the `512 + entry_count × 128` body bytes |
| 40 | 464 | zero |
| 504 | 4 | gate CRC-32 over bytes `0..504` |
| 508 | 4 | zero |

A gate is valid only when: its magic and version are known; its copy index equals its physical
position; its `StoreId` equals the superblock's; its gate CRC checks; its entry count is within
capacity; and the body CRC equals a fresh CRC of the body it describes. There is no partially valid
gate and no repair path. The gate CRC already covers the body-CRC field, so the body CRC needs no
mirror or complement of its own.

**Invalidating** a gate means writing 512 zero bytes over exactly that block and synchronizing. An
all-zero gate fails magic and CRC, so invalidation needs neither a sentinel value nor a
read-modify-write.

### 5.5 Commit

A commit is the only durable state transition an object ever undergoes. Payload bytes are written and
synchronized **before** it begins; extents that hold uncommitted bytes are held in RAM only, so a cut
at any point before step 3 leaves those bytes anonymous and their extents free at the next mount.

Let `A` be the copy the store is **currently serving** — the one mount selected in §5.6, or the one
the last commit wrote — and `B` the other. The target is defined by what the store is serving, not by
which gate has the greater sequence, because those differ: mount falls back to the older copy when the
newer one's gate is valid but its body fails, and a commit that then overwrote the copy it was serving
would leave the card with no valid catalog at all.

1. Invalidate `B`'s gate; synchronize.
2. Write `B`'s body — one header block, then `ceil(entry_count / 4)` entry blocks — with a commit
   sequence one greater than the highest any gate on this card has carried; synchronize. Mount notes
   that high-water mark from **both** gates it parsed, valid body or not, so the sequence a client
   caches never repeats even after a fallback.
3. Write `B`'s gate; synchronize.

After step 3 the store's truth is `B`. A cut anywhere before step 3 completes leaves `A` valid with
the greater sequence and `B` invalid: the commit did not happen, and every byte it would have made
visible is anonymous again. A cut during step 3 corrupts only the gate page, which fails its CRC.

Step 1 exists because step 3 may otherwise leave a gate that certifies a body only half rewritten:
`B`'s old gate would still name an old entry count and an old body CRC, both of which a partially
written body can accidentally satisfy in the prefix the count selects. Invalidating first makes
"body not yet certified" the only intermediate state.

One commit carries a batch of entry mutations — put an entry, remove an entry — and applies them
atomically, because the mechanism rewrites the whole live prefix regardless. Weather retention (put
the new head, set `RETAINED` on the displaced entry) and finalising a ride (clear `RECORDING`, trim
the ranges, set length and CRC) are each one commit.

The cost is `ceil(n/4) + 3` block writes and three synchronizations: about **15–20 ms** at a few
hundred entries, dominated by the synchronizations. That budget assumes commits are rare —
publication events, not a running log. If a future feature needs to commit more than about once per
second, the whole-catalog copy is the thing to re-examine, not the thing to work around.

### 5.6 Mount

1. Read superblock A block 0; on failure read superblock B. Neither valid ⇒ the card is not a flat
   store: initialization (§8) is the only transition, and it is destructive and explicit.
2. Read both gate blocks (2 reads) and note the highest commit sequence either carries. Neither gate
   valid ⇒ read-only, evidence preserved: a valid superblock implies a catalog was written before it
   (§8), and §5.5 never leaves both gates invalid, so this is media damage rather than any state the
   store can produce. Two valid gates at equal sequences is likewise corruption and mounts read-only.
   Otherwise take the valid gate with the greater sequence.
3. Read that copy's body — `1 + ceil(entry_count / 4)` blocks — and check the body CRC and every
   structural rule of §5.3. On failure, fall back to the other copy and repeat. Both bad ⇒ read-only,
   evidence preserved, no repair. The copy that succeeds is the one the store **serves**, and §5.5's
   next commit targets the other one.
4. Build the free-extent bitmap: every extent free, then mark the ranges of every entry used. An
   overlap is a structural failure of that copy.
5. If exactly one entry carries `RECORDING`, read the 16 ride journal slots and recover the ride
   (§7.3). Otherwise the journal is not read at all.

Those five steps are the whole of mount. There is **no journal replay** — the catalog copy *is* the
commit; **no garbage collection** — an extent is free exactly when no entry names it; and **no
recovery scan** — nothing on the card can be committed that the winning gate does not already
describe. On a card with no ride in progress a mount reads at most 3 blocks plus the live catalog
prefix, which is why boot is about 100 ms.

One thing that reads like mount is deliberately outside it: reconciling an update that armed before
the last reboot, which may remove an orphaned rollback reserve
([`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §4). That is device policy running against a
mounted store, on the ordinary commit path, and it is not part of bringing the card up.

### 5.7 Vectors

A catalog holding two entries: route `ObjectId 1` at `Revision 3`, and a ride recording under
`ObjectId 2` at `Revision 1`. `StoreId` as in §4.1; commit sequence `7`; next `ObjectId` `3`.

Header, block 0 of the copy (bytes `42..512` are zero):

```
0000  46 53 43 54 01 00 80 00 8F 2C 41 D9 6B 07 4E A3
0010  B1 55 9C 20 7D E8 34 66 07 00 00 00 00 00 00 00
0020  03 00 00 00 00 00 00 00 02 00 00 00 00 00 00 00
```

Entry 0 — route, 1 range (extent 12, 1 extent), 42,137 bytes, payload CRC `0x9C4A7E21`, name
`"Grimsel Loop"`:

```
0000  01 00 00 00 01 0C 00 00 01 00 00 00 00 00 00 00
0010  03 00 00 00 00 00 00 00 99 A4 00 00 00 00 00 00
0020  21 7E 4A 9C 00 00 00 00 0C 00 01 00 00 00 00 00
0030  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0040  00 00 00 00 00 00 00 00 47 72 69 6D 73 65 6C 20
0050  4C 6F 6F 70 00 00 00 00 00 00 00 00 00 00 00 00
0060  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0070  00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
```

Entry 1 — ride, `RECORDING`, 1 range (extent 13, 32 extents = a 32 MiB reserve), length and CRC zero,
no name:

```
0000  03 00 01 00 01 00 00 00 02 00 00 00 00 00 00 00
0010  01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
0020  00 00 00 00 00 00 00 00 0D 00 20 00 00 00 00 00
0030  ..                                        (zero to 128)
```

The body is those 768 bytes (`512 + 2 × 128`); its CRC-32 is `0x9C1D23F9`.

Gate of copy A:

```
0000  46 53 43 47 01 00 00 00 8F 2C 41 D9 6B 07 4E A3
0010  B1 55 9C 20 7D E8 34 66 07 00 00 00 00 00 00 00
0020  02 00 00 00 F9 23 1D 9C 00 00 00 00 00 00 00 00
0030  ..                                        (zero to 504)
01F8  B8 31 55 93 00 00 00 00
```

Gate CRC-32 over bytes `0..504` is `0x935531B8`.

## 6. Extent area

The extent area begins at LBA 4096 and is a flat array of **1 MiB** extents, 2048 blocks each.
Extent `k` begins at LBA `4096 + 2048 × k`.

```
extent_count = min(65536, (total_card_blocks - 4096) / 2048)      // integer division
```

The cap is what the entry's `u16` extent index buys: 65,536 extents is 64 GiB of extent area and an
8 KiB resident free bitmap. A larger card leaves its tail unused; that is a deliberate trade of
capacity nobody has asked for against 8 KiB of RAM on a part that has little.

### 6.1 Addressing

An object's payload is the concatenation of its ranges in order. With ranges `(f_i, c_i)` and
cumulative byte starts `s_0 = 0`, `s_{i+1} = s_i + c_i × 2^20`, payload offset `o` lies in the range
`i` with `s_i <= o < s_{i+1}`, and

```
lba   = 4096 + 2048 * f_i + (o - s_i) / 512
inner = (o - s_i) % 512
```

That is the whole read path: at most eight comparisons and two divisions by constants, no indirection
block, no chain walk, no cache. It is what `fat_extents.rs` was built to fake on top of a filesystem
(#500), and it is why that file goes away in FS6.

Because every range begins on a 1 MiB boundary and 1 MiB is a multiple of the 16,384-byte program
page, **any payload offset that is a multiple of 16,384 maps to a page-aligned LBA**. §7 relies on
that.

Example, with the route entry of §5.7 (range `(12, 1)`): payload offset 40,960 is in range 0, so
`lba = 4096 + 2048 × 12 + 40960 / 512 = 4096 + 24576 + 80 = 28,752`, inner offset 0.

### 6.2 Allocation

Allocation is first-fit over the free bitmap, in ascending extent order, and the result is at most 8
ranges. An allocation that cannot be expressed in 8 ranges is **refused** — the caller sees a
refusal, never a partial object and never a rewritten card. Fragmentation's worst case is therefore a
refused allocation, never corruption. There is no object mover in format version 1; when a real card
refuses a real allocation, that is when one gets designed.

An allocation is RAM state until the commit that names its extents in an entry. It is released by an
explicit cancel, by the store's drop, and by mount — which rebuilds the bitmap from the catalog and
so cannot see it at all.

**Extents named by no entry are free, immediately.** A commit that replaces an object frees the old
extents at the moment its gate lands. The one qualification is a reader: while an open handle names
an entry, the store keeps that entry's extents out of the allocator even after a commit has removed
it, and returns them when the last handle closes. This hold is RAM-only and needs no durable record —
after a reboot the extents are free, and there is no reader left to be surprised.

## 7. Ride journal

A ride grows for hours. Committing it every ten seconds would violate the commit-rate budget of §5.5
outright, and not committing it at all would risk hours of track. The journal is the one mechanism
that resolves that, and it is the only place in this format where bytes become durable without a
commit.

The region is 16 slots of 32 KiB at LBA 1088, slot `k` at LBA `1088 + 64 × k`. A slot is two program
pages, written in one shot and covered by one CRC: a cut corrupts a page, the CRC fails, and the slot
is skipped. There is no gate.

### 7.1 Slot

Block 0 of the slot is the header; blocks `1..64` are the tail, 32,256 bytes.

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | magic `FSRJ` (`46 53 52 4A`) |
| 4 | 2 | format version, `1` |
| 6 | 2 | slot index, `0..=15`, equal to its physical position |
| 8 | 16 | `StoreId` |
| 24 | 8 | ride `ObjectId` |
| 32 | 8 | ride entry `Revision` |
| 40 | 8 | checkpoint sequence, from `1` |
| 48 | 8 | flushed length, a multiple of 16,384 |
| 56 | 4 | tail length, `0..=32256` |
| 60 | 4 | payload CRC-32 over `[0, flushed length + tail length)` |
| 64 | 32 | 8 × extent range, copied verbatim from the ride's catalog entry |
| 96 | 408 | zero |
| 504 | 4 | slot CRC-32 over the whole 32,768-byte slot with this field zero |
| 508 | 4 | zero |
| 512 | 32,256 | tail bytes, then zero to the end of the slot |

The ride's payload at this checkpoint is `extents[0 .. flushed length]` followed by the slot's first
`tail length` tail bytes. The total is `flushed length + tail length`; it is derived, not stored.

The extent ranges are copied into the slot as a **cross-check**, not to save a lookup — recovery has
the catalog entry in hand already, since that entry's `RECORDING` flag is what sent it here. A slot
whose ranges differ from that entry's is rejected, which is what stops a slot left by an earlier ride
over reused extents from being read as this one's.

### 7.2 Recording

Ride start is one commit: allocate a **32 MiB reserve** — 32 extents, roughly 400 hours at the ride
payload's real byte rate — put an entry of kind `ride` with `RECORDING` set, length and CRC zero. No
further commit happens until the ride ends, which is why the reserve is taken up front and why a start
that cannot get it in 8 ranges fails rather than recording into a budget it might outgrow.

Then, on a fixed cadence of **10 seconds**:

1. Append the new points to the in-RAM tail.
2. While the tail holds at least 16,384 bytes, write one 16,384-byte payload page at payload offset
   `flushed length` — page-aligned by §6.1, written once, never rewritten — synchronize, and move
   those bytes from the tail to `flushed length`.
3. Write slot `checkpoint sequence mod 16` in one shot; synchronize.

Step 2 before step 3 is the ordering that matters: a payload page is written only when every byte in
it is already in a slot on the card, and once written it is never touched again. Step 3 then records
that it is durable. A cut between the two leaves the previous slot authoritative — its `flushed
length` is one page behind, its tail still holds those bytes, and recovery simply rewrites the page.

The binding limit on the tail is the slot's 32,256-byte tail area, not the `u32` field that measures
it. Step 2 leaves at most 16,383 bytes behind, so an interval may add **15,873 bytes** before the tail
would not fit — three orders of magnitude above the roughly 200 bytes ten seconds of riding produces.
A slot claiming a tail above 32,256 is invalid.

Ride end: flush whatever the tail still holds — a partial page, whose bytes are all in the last slot,
so a cut during it is recovered by rewriting — then one commit that clears `RECORDING`, sets the
final length and payload CRC, and trims the ranges to `ceil(length / 1 MiB)` extents, freeing the
rest of the reserve. The 16 slot headers are then zeroed. A cut during that zeroing is harmless: no
entry carries `RECORDING`, so §5.6 never reads them.

### 7.3 Recovery

Read the 16 slots. A slot is a candidate when its magic, version, slot index, `StoreId`, slot CRC and
tail-length bound all check, and its `ObjectId`, `Revision` and ranges equal those of the entry
carrying `RECORDING`. Take the candidate with the greatest checkpoint sequence. That is the whole
mandatory decision: at most 16 slot reads, performed only when an entry says a ride was recording.

**The payload CRC is a seed, not a verification obligation.** Its normative role is to give the
resumed session the running CRC it needs so that the commit ending the ride can state the whole ride's
CRC without re-reading it. A recovery MAY re-read `extents[0 .. flushed length]` and check the CRC of
that prefix followed by the tail against the field — a host harness should, and FS3's crash matrix
will — but a device MUST NOT make that read a precondition of mounting: after a long ride the prefix
is tens of megabytes, and a full-prefix read on every boot is exactly the recovery scan this format
does not have. A device that does check SHOULD bound the read to the last flushed page, which is the
only region a cut can have damaged.

Recovery then hands the store the recovered tail and `flushed length`, and recording resumes from
there or the rider finalises it. **Recording resumes at checkpoint sequence `recovered + 1`**, never
at `1`: restarting the count would leave the stale slots of this same ride carrying greater sequences
than the resumed session's, and the next recovery would pick one of them and roll the ride back.

### 7.4 Loss cap

Every power cut loses at most the points recorded since the newest **valid** slot. A cut that tears
the slot being written costs the interval before it as well. With the 10-second cadence the worst
case is therefore **two intervals, about 20 seconds of track**, and the ride itself — every byte
before that point — cannot be lost, because a payload page is only ever written from bytes a slot
already holds and is never rewritten.

### 7.5 Vector

Slot 3 of a ride under `ObjectId 2`, `Revision 1`, checkpoint sequence 41, 245,760 bytes flushed (15
pages), 3,712 bytes of tail, ranges as in §5.7. The tail bytes of this vector are
`tail[i] = (i × 7 + 3) mod 256` for `i < 3712` and zero after.

```
0000  46 53 52 4A 01 00 03 00 8F 2C 41 D9 6B 07 4E A3
0010  B1 55 9C 20 7D E8 34 66 02 00 00 00 00 00 00 00
0020  01 00 00 00 00 00 00 00 29 00 00 00 00 00 00 00
0030  00 C0 03 00 00 00 00 00 80 0E 00 00 C7 03 1B 5E
0040  0D 00 20 00 00 00 00 00 00 00 00 00 00 00 00 00
0050  ..                                        (zero to 504)
01F8  D6 6B E5 66 00 00 00 00
0200  03 0A 11 18 1F 26 2D 34 3B 42 49 50 57 5E 65 6C   (tail begins)
```

The slot CRC-32 over the whole 32,768 bytes, with bytes `504..508` zero, is `0x66E56BD6`. The payload
CRC field `0x5E1B03C7` is a declared value covering payload bytes `[0, 249472)`, which this vector
does not carry.

## 8. Initialization

Initialization is explicit, destructive, and the only transition into this format. A card that is not
already a valid flat store is never written to implicitly.

1. Draw a fresh `StoreId` from the device CSPRNG.
2. Zero the gate block of catalog copy B; synchronize.
3. Zero the 16 ride-journal slot header blocks; synchronize.
4. Write catalog copy A's body — header with commit sequence `1`, next `ObjectId` `1`, entry count
   `0` — then its gate; synchronize each.
5. Write superblock copy A, then copy B; synchronize.

**The superblocks go last, and that ordering is the invariant.** A valid superblock therefore implies
a valid catalog: every cut before step 5 leaves a card §5.6 step 1 classifies as *not a flat store*,
which is the destructive-initialization path and not a data-loss one. Writing them first would leave a
card that mounts — a valid `StoreId` and no catalog at all — which §5.6 step 2 can only answer with
read-only.

A mount after step 5 finds copy A valid and copy B invalid, an empty catalog, and every extent free.
There is no migration path from any earlier layout: an old card is re-initialized, and every object
on it is gone.

## 9. Capacities

Format constants and product limits, gathered for reference. Each is normative in the section that
defines it, not here.

| Limit | Value |
| :-- | --: |
| Program page | 16,384 bytes |
| Extent size | 1 MiB (2048 blocks) |
| Extent-area start | LBA 4096 |
| Extents addressable | 65,536 (64 GiB of extent area) |
| Free bitmap, resident | 8 KiB |
| Catalog copies | 2 × 256 KiB |
| Catalog entries | 1916 |
| Catalog entry | 128 bytes |
| Extent ranges per object | 8 |
| Display name | 48 UTF-8 bytes |
| Ride journal slots | 16 × 32 KiB |
| Ride journal tail per slot | 32,256 bytes |
| Ride checkpoint cadence | 10 s |
| Ride reserve at start | 32 MiB (32 extents) |
| Rides recording at once | 1 |
| Retained previous revisions per object | 1 |
| Commits per second, design budget | ~1 |

## 10. What is not here

Each of these was in the format this one replaces, and each is absent for one reason.

- **No journal replay.** The catalog copy *is* the commit; there is no separate log to apply.
- **No garbage collection.** An extent is free exactly when no entry names it, so freeing is what a
  commit already did.
- **No recovery scan.** Nothing durable exists outside what the winning gate describes; there is
  nothing to look for.
- **No per-record gates.** Only the catalog spans more than one program page, so only the catalog
  needs one.
- **No `GenerationId`.** Uncommitted bytes are anonymous; committed bytes are named by an entry.
- **No `OperationId` and no durable result record.** The catalog is the result: a client that lost
  the link reconciles against it ([`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §3.4).
- **No partition table and no filesystem.** The card is not user-accessible and no host reads it.
- **No migration.** An old card is re-initialized.
