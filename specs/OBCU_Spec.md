# OBCU File Format Specification (v2)

OBCU (OpenBikeComputer Update) is the byte format of a **field firmware update**. It has three
parts, all defined here and all implemented by the shared `no_std` crate `firmware/obc-dfu`, which
the `obc-boot` bootloader links:

1. **The update-image container** (§1) — the update-package object on the card: a
   fixed **64-byte header**, the raw application image, and (v2) a **signature
   trailer**.
2. **The boot-state page** (§2) — the CRC-framed blob in a dedicated **4 KB RRAM
   page**, the sole handoff channel between the app (the *armer*) and the bootloader
   (the *installer*).
3. **The storage-blob stage carve** (§3) — the CRC-framed **20 KB RRAM
   carve** through which the armer hands the bootloader the sEMMC soft-peripheral
   image it boots the card with.

It shares the conventions of the [`OBCM`](OBCM_Spec.md) map and [`OBCR`](OBCR_Spec.md) route
formats: **little-endian** integers throughout, an explicit magic, version and CRC frame, and **no
runtime discovery** — every field is at a fixed or self-describing offset.

All multi-byte integers are **little-endian**. The integrity check everywhere is
**CRC-32/IEEE** (reflected polynomial `0xEDB88320`, init/xor-out `0xFFFFFFFF`, check
value `crc32("123456789") == 0xCBF43926`).

---

## 1. Update-image container

```
[OBCU header]   (64 bytes, fixed)
[raw image]     (image_len bytes — the app's objcopy -O binary output, vector table first)
[signature]     (sig_len bytes — v2 only; 64 for Ed25519, absent when sig_scheme = 0)
```

The container is one **update-package object** (kind `7`) in the flat store
([`FLAT_Store_Format.md`](FLAT_Store_Format.md) §3.1). A client uploads it; the card carries no
filesystem and no sideload path. It is produced by the `obc-mkimage wrap` / `obc-mkimage sign` host
tool and consumed by the app-side armer, which validates the header, the full image CRC, **and the
signature** before arming.

Two container shapes exist, distinguished by the header's `Sig Scheme` field — **not**
by Header Version, which stays `1` forever (§1.2):

| Shape | `Sig Scheme` | Bytes | Produced by | Armer verdict |
| :-- | :-- | :-- | :-- | :-- |
| **v1**, unsigned | `0` | `64 + image_len` | `obc-mkimage wrap` with no seed; the device's own rollback snapshot | **rejected** (§1.4) |
| **v2**, Ed25519 | `1` | `64 + image_len + 64` | `obc-mkimage wrap --sign-seed` / `sign` | accepted iff the signature verifies |

### 1.1 Header (64 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCU"` |
| 4 | Header Version | 2 | `uint16` | `0x0001` — **in v2 too** (§1.2); readers reject any other value |
| 6 | Reserved | 2 | — | `0` |
| 8 | Image Len | 4 | `uint32` | Bytes of the raw image following the header |
| 12 | Image CRC-32 | 4 | `uint32` | CRC-32/IEEE over the raw image **only** |
| 16 | FW Version | 32 | `char[32]` | UTF-8 `git describe` string, NUL-padded |
| 48 | Sig Scheme | 2 | `uint16` | `0` unsigned (v1) · `1` Ed25519 (v2, §1.3). The v1/v2 discriminator |
| 50 | Sig Len | 2 | `uint16` | Bytes of the signature trailer: `0` when Sig Scheme = 0, `64` for Ed25519 |
| 52 | Reserved | 8 | — | `0` |
| 60 | Header CRC-32 | 4 | `uint32` | CRC-32/IEEE over header bytes `0..60` |

`Sig Scheme` and `Sig Len` occupy the first four bytes of the 12-byte region v1
reserved "for a future signature-scheme marker"; the remaining eight stay reserved and
MUST be zero. An unsigned v2-era container (both fields `0`) is therefore **byte-identical
to a v1 container** — including its Header CRC-32.

**Decode rule** (`ImageHeader::decode(&[u8; 64]) -> Option`): return `None` on bad
magic, a Header Version other than `1`, or a Header CRC-32 that doesn't match bytes
`0..60`; otherwise `Some`. This is the settings-store convention — a **valid CRC ⇒
`Some`**, and a version change is a hard reject, never a silent migration. The rule is
**unchanged from v1 and MUST stay unchanged**: `Sig Scheme`/`Sig Len` are decoded but
never validated here, because a decoder that started rejecting unfamiliar scheme values
would no longer read v1 and v2 alike (§1.2). Whether a decoded container may be
*installed* is the armer's policy call (§1.4), not the codec's. The
raw-image CRC (offset 12) is verified **separately**, against the staged image
bytes, by whoever is about to trust them (the armer over the file, the bootloader
over the resolved extents).

`FW Version` is read back with trailing NULs trimmed; an over-long version string is
truncated to 32 bytes on a UTF-8 char boundary at wrap time (never mid-codepoint).

`Image Len` must not exceed **`MAX_IMAGE_LEN` = 2,023,424** bytes. That is the app slot itself —
`__semmc_stage_base - __app_slot_base` in §3's map — and nothing less: the bootloader's install
engine erases and writes exactly that span, so an image which fits the slot is an image the device
can install. `obc-mkimage wrap` refuses a larger image. The **whole container** is
`64 + Image Len + Sig Len` bytes, so a transfer that gates on a length gates at the **container**
ceiling `MAX_CONTAINER_LEN` = `MAX_IMAGE_LEN + 64 + 64` = 2,023,552: a raw image at the cap must not
be refused for its own framing. A container is delivered as a `PUT` of kind `7`
([`FLAT_Store_Protocol.md`](FLAT_Store_Protocol.md) §3.6, §4). Bytes past
`64 + Image Len + Sig Len` in the delivered file are ignored (§2.3).

### 1.2 Header Version stays 1 — the flash-once bootloader guarantee (normative)

`obc-boot` lives in a 32 KB region, is flashed **once by SWD probe**, and is **never
updated by DFU** (that is what makes it the dependable half of the boot chain). Its
copy of the header decoder therefore never changes, and it hard-rejects any Header
Version but `1`. A v2 container that bumped that field would be unparseable to every
bootloader already in the field: the device would decode `Armed`, fail the install's
header check, and fall back to the old app — forever, on every future update.

So it does not bump. **A v2 container MUST carry Header Version `0x0001`.** The
compatibility argument, field by field — this is the complete set the bootloader's
install engine consumes (it decodes the container header off the card, compares it
against the `ImageHeader` embedded in the boot-state page's `StagedRef`, CRCs the next
`Image Len` bytes, and flashes exactly those):

| What the fielded bootloader reads | Offset | v1 | v2 |
| :-- | :-- | :-- | :-- |
| Magic | `0..4` | `OBCU` | **identical** |
| Header Version | `4..6` | `1` | **identical** |
| Image Len | `8..12` | raw image bytes | **identical** |
| Image CRC-32 | `12..16` | over the raw image | **identical** |
| FW Version | `16..48` | NUL-padded string | **identical** |
| Header CRC-32 | `60..64` | over bytes `0..60` | recomputed — it now covers the marker |
| Image bytes | `64 .. 64+Image Len` | the raw image | **identical, same offset** |
| Anything past the image | — | "ignored" | the signature trailer — still ignored |

Only two regions differ: bytes `48..52`, which v1 pinned to zero and explicitly
reserved for exactly this, and the header CRC that covers them. A v1 decoder never
looks at `48..60`, so a v2 header decodes to precisely the same `(Image Len, Image
CRC-32, FW Version)` triple; the bootloader's header-equality check still matches
because the app writes the same 64 bytes into the boot-state page that sit on the card;
and nothing the bootloader flashes moved. Consequently **the bootloader needs no change
to install v2 images, and MUST NOT be required to verify signatures.**

### 1.3 The signature (normative)

`Sig Scheme` = `1` means **Ed25519** (RFC 8032, the standard SHA-512 / Curve25519
parameters) and `Sig Len` = `64`. The trailer at file offset `64 + Image Len` holds the
signature's 64 bytes, `R ‖ S`, exactly as RFC 8032 encodes them.

The signed message is **domain-separated** and **binds the labelling**:

```
signed_message =
      "OBCUv2-sig\0"            11 bytes — the context, ASCII, trailing NUL included:
                                  4F 42 43 55 76 32 2D 73 69 67 00
   || FW Version                 32 bytes — header bytes 16..48, raw and NUL-padded
   || Image Len                   4 bytes — header bytes  8..12, uint32 little-endian
   || image[0 .. Image Len]      the raw application image, unmodified
```

Total length `47 + Image Len`. `Image CRC-32` and `Sig Scheme` are **not** covered: the CRC is a
pure function of image bytes that are covered, and a rewritten scheme value only moves the container
into a bucket the armer rejects (§1.4).

Signing MUST be **deterministic**: no per-signature randomness beyond RFC 8032's own
seed-derived nonce. That makes a release artifact byte-reproducible and lets a signed
container be a committed test vector.

The trusted **public key** is compiled into the firmware image
(`firmware/obc-dfu/keys/obcu-release.pub`, one line of 64 hex characters). Key rotation
is therefore a firmware change by construction: a device trusts exactly the key its own
build carries. See `firmware/obc-dfu/keys/README.md`.

### 1.4 Armer acceptance rules (normative)

The app-side armer's staging scan MUST reject a container unless **all** of the
following hold. The order is normative — each check is cheap relative to the next, and
it determines which message the rider sees:

1. The 64-byte header decodes per §1.1 → else *bad header*.
2. `0 < Image Len ≤ MAX_IMAGE_LEN` → else *oversize*.
3. `Sig Scheme` = `1` **and** `Sig Len` = `64` → else ***unsigned***. This one bucket
   covers a plain v1 container, a marker-cleared v2 container, and any future scheme
   this firmware cannot verify. **An unsigned container is rejected, not merely
   flagged**: if a v1 wrapper were still installable, an attacker would never bother
   forging a signature — they would omit it, and the whole scheme would be decorative.
4. The file is at least `64 + Image Len + Sig Len` bytes → else *truncated*.
5. The trailer parses as an Ed25519 signature under the trusted key → else *bad
   signature*. (Checked before the image is read, so a junk trailer costs nothing.)
6. CRC-32 over the image body matches `Image CRC-32` → else *bad CRC*.
7. Ed25519 verification over §1.3's message succeeds → else *bad signature*.

Steps 6 and 7 run over a **single streaming pass** of the image: the same bytes feed the CRC and
the signature hash, so verification adds no second read of the card and no image-sized buffer.
Corruption (6) is reported **before** trust (7).

The bootloader performs **no signature check** (§1.2). Its guarantee is verify-before-erase by CRC
over the raw extents, and always leave a bootable image. Signature verification is an
*authorization* gate on arming, not a second integrity gate on flashing.

---

## 2. Boot-state page

One CRC-framed blob written to a dedicated **4 KB RRAM page** (`PAGE_LEN = 4096`).
It is the only channel between the app and the bootloader. RRAMC writes 16-byte
lines, so the **encoded length is always a multiple of 16** (guaranteed by
construction and a compile-time assert), and the armer writes whole lines with no
read-modify-write.

The blob is a fixed 16-byte header, a tag-specific payload, zero padding, and a
trailing whole-blob CRC-32. `blob_len` (offset 8) is the total encoded length
including the CRC; the CRC covers bytes `0 .. blob_len − 4` (the padding included).

### 2.1 Blob header (16 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCB"` |
| 4 | Format Version | 2 | `uint16` | Writers emit `0x0002`; readers accept `0x0001` and `0x0002` |
| 6 | State Tag | 1 | `uint8` | `0` Idle · `1` Armed · `2` Trial |
| 7 | Reserved | 1 | — | `0` |
| 8 | Blob Len | 4 | `uint32` | Total encoded length, incl. CRC; a multiple of 16 |
| 12 | Generation | 4 | `uint32` | Bumped on every arm; `0` for Idle |

`Generation` is a diagnostic breadcrumb, **not** a replay guard. It is bumped on every arm, carried
for `Armed` and `Trial`, and recorded inside the `Idle` payload's **Last Outcome** record (§2.2).
Its one consumer is the app's boot-outcome reconcile, which matches the recorded generation against
the arm marker it left behind. Nothing compares generations to *reject* a page, and `Idle` pins the
header field to `0`, so the counter is not monotonic across a cycle (`Idle 0 → Armed 1 → Idle 0`).
A torn, blank or stale page is caught by the CRC frame and decodes to `Idle` regardless of
generation.

**Version compatibility (normative).** Readers **MUST accept both** `0x0001` and `0x0002`; writers
**MUST emit** `0x0002`. The `Armed` and `Trial` payload layouts are byte-identical across the two
versions; a `0x0001` `Idle` body ends after the installed option (§2.2) and decodes with no Last
Outcome — the decoder gates this on the version field and MUST NOT parse the zero padding after a v1
payload as an outcome record. The bootloader is flashed once by probe and is **not** updated by DFU,
so a fielded bootloader keeps writing `0x0001` pages after the app updates. Both sides therefore
read both versions. The one degraded pair is a `0x0001` bootloader reading a `0x0002` `Armed` page:
it cannot decode it, falls back to `Idle` and jumps, so the install never starts and the app reports
a not-started failure. It is recoverable by reflashing the bootloader and is never a revert loop.

### 2.2 Payload by State Tag

**Tag `0` — Idle** (`installed: Option<ImageHeader>, last_outcome: Option<LastOutcome>`):
the header of the running image (for the UI and to seed a rollback snapshot), plus the
recorded outcome of the arm that produced this `Idle`.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 16 | Has Installed | 1 | `uint8` | `0` none · `1` a header follows |
| 17 | Installed Header | 64 | `ImageHeader` | Present only when Has Installed = 1 (§1.1) |
| … | Has Outcome | 1 | `uint8` | `0` none · `1` a Last Outcome record follows |
| … | Outcome Kind | 1 | `uint8` | Present only when Has Outcome = 1: `0` Installed · `1` RolledBack · `2` StageRejected · `3` ArmAbandoned |
| … | Outcome Generation | 4 | `uint32` | Present only when Has Outcome = 1: the `Generation` of the arm this outcome belongs to |

The **Last Outcome** record is what the bootloader's engine (and the app's trial
confirm) writes into the `Idle` it lands on so the *next* boot reads a fact rather than
inferring one from version strings: `Installed` = the staged image is now running
(a first-install trial accepted, or a rollback that kept the freshly-flashed image
because its snapshot was unreadable); `RolledBack` = an unconfirmed trial was restored
to its snapshot; `StageRejected` = the staged image failed verification before the app
slot was erased; `ArmAbandoned` = the bootloader gave up on an `Armed` card it could not
read within a bounded retry budget and, **because nothing had been erased yet**, cleared
the arm and booted the intact old app (§2.4). `Outcome Generation` lets the app's
boot-outcome reconcile bind the outcome to the arm marker it left before the install
reboot. `Has Outcome = 0` is a plain steady-state `Idle`, a fresh device, or an `Idle`
written by a `0x0001` writer (whose body ends after the installed option — see the
version-compatibility rule in §2.1). The record is absent from the `Armed`/`Trial`
payloads.

**Tag `1` — Armed** (`update: StagedRef, rollback: Option<StagedRef>`): an update is
staged; the installer flashes it.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 16 | Update | var | `StagedRef` | The staged image to install (§2.3) |
| … | Has Rollback | 1 | `uint8` | `0` none · `1` a `StagedRef` follows |
| … | Rollback | var | `StagedRef` | Snapshot of the outgoing image; present only when Has Rollback = 1 |

**Tag `2` — Trial** (`installed: ImageHeader, rollback: Option<StagedRef>`): a
freshly-installed image is on its single trial boot.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 16 | Installed Header | 64 | `ImageHeader` | The running image's header (§1.1) |
| 80 | Has Rollback | 1 | `uint8` | `0` none · `1` a `StagedRef` follows |
| 81 | Rollback | var | `StagedRef` | Snapshot to restore on an unconfirmed trial |

After the payload the blob is zero-padded so that `blob_len` (payload end + 4-byte
CRC, rounded up to a 16-byte line) is a multiple of 16. The final 4 bytes at
`blob_len − 4` are the whole-blob CRC-32 over bytes `0 .. blob_len − 4`.

### 2.3 StagedRef (variable)

A staged image resolved to raw SD block extents.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| +0 | Header | 64 | `ImageHeader` | The staged image's OBCU header (§1.1), self-validating |
| +64 | Len | 4 | `uint32` | Total raw image length, bytes (matches `Image Len`) |
| +68 | Image CRC-32 | 4 | `uint32` | CRC-32/IEEE over the whole raw image — the verify-before-erase check |
| +72 | Extent Count | 2 | `uint16` | Number of extents, `0 … MAX_EXTENTS` |
| +74 | Extents | 8 × count | `Extent[]` | The block runs, in image order |

Each **Extent** is 8 bytes:

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| +0 | Start Block | 4 | `uint32` | First **absolute** 512-byte SD block of the run |
| +4 | Blocks | 4 | `uint32` | Number of 512-byte blocks in the run |

**`MAX_EXTENTS` = 8.** A stored object holds at most eight extent ranges
([`FLAT_Store_Format.md`](FLAT_Store_Format.md) §5.3) and one range is one contiguous block run, so
eight is the whole of what an arm can ever resolve. The armer errors out past this rather than
truncating the chain, and the decoder rejects an Extent Count above `MAX_EXTENTS`.

`Len` and `Image CRC-32` deliberately duplicate the embedded header's `Image Len`
and `Image CRC-32` (so the installer reads them without re-decoding the header) and
**MUST match** them; decoders MUST reject a `StagedRef` where either pair disagrees
(a diverging record was never built from one coherent image).

**What the extents cover.** The chain locates the **whole staged object**: the armer resolves the
container as-is, so the chain's byte stream begins with the object's own 64-byte OBCU header (§1.1)
followed by the raw image. Everything past `64 + Len` — the v2 signature trailer (§1.3), and the
slack in the object's last extent — is ignored by the installer.
`Len` / `Image CRC-32` remain **raw-image** values: the installer's verify pass reads
the leading 64 bytes only to check they decode to exactly the `Header` recorded above,
then CRCs the next `Len` bytes — and its flash pass writes those same `Len` bytes (the
container header is skipped, never flashed; the trailer is never even read) to the app
slot. This is normative for both the armer and the bootloader; the skip arithmetic lives once, in
`obc-dfu`'s install engine.

**The rollback snapshot is unsigned.** The armer's copy of the running image, written from the app
slot into the **rollback reserve** ([`FLAT_Store_Format.md`](FLAT_Store_Format.md) §3.1, kind `8`)
before an install, is a **v1/unsigned** container (`Sig Scheme` = `0`). The device cannot
reconstruct the original release signature from slot bytes alone, and nothing needs one: the
snapshot never passes through the armer's scan (§1.4), and the bootloader's rollback path validates
it by CRC like everything else. Marking it signed with no trailer behind it would make the bytes lie
to `obc-mkimage inspect`. The `StagedRef` the armer records for the snapshot carries the same
unsigned header, so the installer's header-equality check still matches.

### 2.4 Decode rule and boot decision

**`BootState::decode(&[u8]) -> BootState`** returns `Idle { installed: None,
last_outcome: None }` for **anything** but a clean read of a known format: too short,
bad magic, a Format Version other than `1` or `2` (readers accept both; writers emit
`2` — the normative compatibility rule in §2.1), a `Blob Len` that is out of range or
not a multiple of 16, a failed whole-blob CRC, an unknown State Tag, an unknown
Outcome Kind, an Extent Count over `MAX_EXTENTS`, a `StagedRef` whose redundant
`Len`/`Image CRC-32` disagree with its embedded header (§2.3), or a nested
`ImageHeader` whose own CRC fails. A version-`1` page decodes with `0x0001` semantics:
`Armed`/`Trial` identically, `Idle` with `last_outcome: None`. This is the torn-write
safety net — the bootloader always receives a sane state.

The bootloader turns the decoded state into an action with the pure function
**`decide(&BootState) -> BootDecision`**:

| State | Decision |
| :-- | :-- |
| `Idle` | `Jump` — run the app |
| `Armed` | `Install(update)` — verify + flash the staged image, then write `Trial` and jump straight into it (the one trial boot) |
| `Trial` with a rollback snapshot | `Rollback(snapshot)` — flash the snapshot back |
| `Trial` with no snapshot | `AcceptAndClear` — accept the running image (first-install case) and clear to `Idle` |

A healthy app confirms by writing `Idle { installed }` mid-run, so a `Trial` still
present at the next bootloader entry is by definition *unconfirmed* — which is
exactly why it means "roll back". Load-bearing corollary: after writing `Trial` the
install path must **jump into the new image, never reset** — a reset would re-enter
the bootloader with the fresh `Trial` and roll the image back before it ever ran. A
hardware watchdog guarantees a wedged trial boot becomes the next boot: the
bootloader starts the dog itself — with the app's exact config; the shared 24 s period is
`obc_dfu::WDT_TIMEOUT_TICKS` — immediately before the trial jump, so the guarantee holds even on a
cold power-on where no watchdog was running yet. On the warm-reset arm path the app's
already-running dog is instead adopted and fed through the install, so a slow install is never cut
down mid-flash. A plain `Idle` boot never touches the watchdog.

**Unreadable-card handling.** A card the bootloader cannot read is retried
with a growing backoff, but *how long* depends on whether the app slot has been touched —
the same verify-before-erase line that governs everything else. **Before** the engine's
flash pass begins (a bring-up failure, or an SD error during the verify pass of an
`Armed` install) the old app is still intact, so after a bounded budget of pre-erase
failures (~a minute) the bootloader **abandons** the arm: it writes `Idle` — carrying the
outgoing image's header forward exactly as a rejected stage does — with an `ArmAbandoned`
Last Outcome, and boots the intact old app. **Once the flash pass has begun** (the slot may be
half-written) and for a `Rollback` (whose
trial image is the only bootable thing), an SD error instead retries **forever** — a touched slot is
never abandoned. The retry *count* is a bootloader policy; "abandon writes `Idle` and
`ArmAbandoned`, pre-erase only" is the rule.

---

## 3. Storage-blob stage carve

The microSD card is only reachable through Nordic's **sEMMC soft peripheral** — a
position-independent RISC-V image the FLPR coprocessor executes. The app embeds that image in its
own flash. The 32 KB bootloader cannot, and it must not read it out of the app slot, because the
install engine rewrites the slot while still streaming the staged image from the card. The armer
therefore **stages the blob into a dedicated RRAM carve** the bootloader reads instead.

### 3.1 Layout

A fixed 20 480-byte (`obc_dfu::blobstage::STAGE_LEN`, five 4 KB RRAM pages) region
directly **below the BOOT_STATE page**, taken off the top of the app slot. Nothing else
moves — the app base, BOOT_STATE and SETTINGS keep their addresses:

```
0x0000_0000  obc-boot           32 KB
0x0000_8000  app slot         1976 KB
0x001F_6000  SEMMC_STAGE        20 KB   ← this section
0x001F_B000  BOOT_STATE page     4 KB   (§2)
0x001F_C000  SETTINGS page       4 KB
```

The addresses live only in the linker scripts (`__semmc_stage_base`, `__boot_state_base`). The
carve length matches the RAM carve the image executes in (`SEMMC_CARVE_BYTES`).

### 3.2 Contents

One 16-byte header line (the RRAMC write-line granularity), then the raw image bytes:

| Offset | Size | Field |
| --: | --: | :-- |
| 0 | 4 | Magic `"OBSB"` |
| 4 | 2 | Stage Version, `0x0001`, little-endian |
| 6 | 4 | Blob Len (bytes), little-endian |
| 10 | 4 | CRC-32/IEEE over the Blob Len blob bytes, little-endian |
| 14 | 2 | Reserved (zero) |
| 16 | Blob Len | The soft-peripheral image, byte-exact |

Decode follows the crate-wide rule: **valid CRC ⇒ staged blob, anything else ⇒ "no blob
staged"** (`blobstage::validate_stage`), total over arbitrary bytes.

### 3.3 Armer ordering (normative)

The stage is written **before** the `Armed` page, on the costs-nothing side of §1.4's
commit point: blob body first (16-byte lines, zero-padded tail), the CRC-framed header
line **last**, readback-verified through the same validator the bootloader uses, and
only then the boot-state page write. A power cut anywhere before the page write leaves
nothing armed; a torn stage fails its CRC and reads as "never staged". Corollary: **a
valid `Armed` or `Trial`/`Rollback` page implies a valid stage carve.** The stage is
idempotent — a re-arm with the same app image skips the write — and inert without a
boot-state record, like the rollback snapshot. A stage that cannot be written or
verified aborts the arm (`ArmError::BlobStage`) with the page untouched.

### 3.4 Bootloader validation (normative)

Before executing a staged image on the FLPR, the bootloader validates — in order — the
§3.2 frame, then the image's own `softperipheral_metadata_t` header
(`blobstage::sp_geometry`): soft-peripheral magic, metadata header version 2, comm id
REGIF, not self-booting, the **sEMMC** `softperiph_id` (`0xE33C` — a different soft
peripheral must never be booted as an SD host), the internal footprint consistency
checks, and that the declared image fits the execution carve. The *platform* half of the id word is **not** pinned, so a future blob revision for a newer
platform of the same peripheral stays usable. The VRI offset is taken from the validated metadata,
never hard-coded.

An `Armed` decision whose carve fails validation is **abandoned** like an unreadable card past its
retry budget (§2.4; the slot is untouched). A `Rollback` decision whose carve fails validation parks
(SOS) rather than guessing, and a power cycle retries.

---

## Reference implementation

`firmware/obc-dfu` (`no_std`) implements every part of this document: the container header, the
boot-state page codec and `decide`, the install engine, the stage-carve codec and metadata
validation, the signing message and the streaming Ed25519 verifier, and the armer's acceptance
matrix. `obc-boot` links it but never calls the signature half, so no verifier symbol reaches the
32 KB bootloader image. The host tool `host/obc-mkimage` generates keys and produces, signs and
inspects §1 containers; `inspect` exits non-zero on any failure.
