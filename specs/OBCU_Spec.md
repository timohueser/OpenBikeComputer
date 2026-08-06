# OBCU File Format Specification (v2)

OBCU (OpenBikeComputer Update) is the byte format of a **field firmware update** —
the SD-staged DFU foundation (epic #615), signed since **v2** (epic #773, issue
#997). It has three parts, all defined here and all implemented by the shared
`no_std` crate `firmware/obc-dfu` (host-tested to the same bar as the settings codec,
and linked by the `obc-boot` bootloader, `firmware/obc-boot`):

1. **The update-image container** (§1) — the `UPDATE.BIN` file on the SD card: a
   fixed **64-byte header**, the raw application image, and (v2) a **signature
   trailer**.
2. **The boot-state page** (§2) — the CRC-framed blob in a dedicated **4 KB RRAM
   page**, the sole handoff channel between the app (the *armer*) and the bootloader
   (the *installer*).
3. **The storage-blob stage carve** (§3, epic #1158) — the CRC-framed **20 KB RRAM
   carve** through which the armer hands the bootloader the sEMMC soft-peripheral
   image it boots the card with.

It shares the conventions of the [`OBCM`](OBCM_Spec.md) map and
[`OBCR`](OBCR_Spec.md) route formats so the reader/writer code feels identical:
**little-endian** integers throughout, an explicit magic + version + CRC frame, and
**no runtime discovery** (every field is at a fixed or self-describing offset).

## Design principles

1. **Verify before erase.** Nothing writes the app slot until a CRC has passed over
   the complete staged image (epic #615 safety invariant 1). Both a bad container
   header (§1) and a bad staged-image CRC (referenced from §2) reject the update at
   zero cost.
2. **Torn writes decode to a safe state.** The boot-state page (§2) is CRC-framed
   like the settings blob: **anything that doesn't cleanly decode is `Idle`** — a
   blank page, a half-written line, or a bit-flip the CRC catches all mean "no
   pending update, jump to the app", never a garbage install (invariant 4).
3. **The bootloader has no FAT.** Extents in the boot-state page are **absolute
   512-byte SD block runs**, pre-resolved by the app-side armer, so the installer
   reads raw blocks with no filesystem in the 32 KB bootloader budget. Since the
   storage pivot (#1158) the same budget argument gives the bootloader its card
   *transport*: the sEMMC image arrives pre-staged through §3 rather than compiled in.
4. **CRC-32 is the corruption check; the signature is the trust check.** v1 shipped
   with the CRC alone (physical card access is already root on an open device) and
   reserved header space for "a future signature-scheme marker if internet-sourced OTA
   ever lands". v2 spends it: an image is **Ed25519-signed over a domain-separated
   message** (§1.3) and the app-side armer verifies it **before arming** (§1.4). The
   two checks answer different questions and both remain: the CRC catches a torn
   download and says "damaged"; the signature catches a forged one and says "not
   ours". Nothing about the CRC's role, polynomial, or coverage changed.
5. **A fielded bootloader must keep installing new images.** `obc-boot` is 32 KB,
   flashed once by probe, and never updated by DFU — so the v2 container is designed
   so that a bootloader compiled before v2 existed parses and installs it unchanged
   (§1.2). This is why the signature lives in the reserved region and a trailer rather
   than in a bumped header layout, and why the bootloader does not verify signatures.

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

The file lives at the **card root** as `UPDATE.BIN` (8.3-safe, locked). It is
produced by the `obc-mkimage wrap` / `obc-mkimage sign` host tool and consumed by the
app-side armer, which validates the header, the full image CRC, **and the signature**
before staging.

Two container shapes exist, distinguished by the header's `Sig Scheme` field — **not**
by Header Version, which stays `1` forever (§1.2):

| Shape | `Sig Scheme` | Bytes | Produced by | Armer verdict |
| :-- | :-- | :-- | :-- | :-- |
| **v1**, unsigned | `0` | `64 + image_len` | `obc-mkimage wrap` with no seed; the device's own `ROLLBACK.BIN` snapshot | **rejected** (§1.4) |
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

`Image Len` must not exceed **`MAX_IMAGE_LEN` = 1,480,000** bytes — the L15 DK app
slot (`0x8000 … 0x17B000`) minus a small margin. `obc-mkimage wrap` refuses a larger
image. (The LM20's larger slot is a future mechanical bump.) The **whole container**
is `64 + Image Len + Sig Len` bytes; the BLE/USB `fwImage` transfer (protocol §7.6)
announces that container size, so its announce-time reject gates at the **container**
ceiling `MAX_CONTAINER_LEN` = `MAX_IMAGE_LEN + 64 + 64` = 1,480,128 — a raw image at
the cap must not be refused for its own framing. Bytes past
`64 + Image Len + Sig Len` in the delivered file are ignored (FAT cluster slack, §2.3).

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

Pinned by `obc-dfu`'s `tests/signature.rs`
(`a_fielded_v1_decoder_accepts_a_v2_header_with_identical_fields`, which reimplements
the v1 decoder straight from the table above rather than calling the current code, plus
`the_install_engine_flashes_a_v2_container_unchanged` and
`the_install_engine_treats_v1_and_v2_identically`) and cross-implementation by the
`update-container-v1.bin` / `update-container-v2.bin` fixture pair in `specs/vectors/`.

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

Total length `47 + Image Len`. Every part is load-bearing:

- The **context string** makes an OBCU signature useless anywhere else. A key that also
  signs in some other protocol can never produce a cross-valid signature, because no
  other message format begins with these eleven bytes. The NUL terminates the context
  unambiguously, so no `FW Version` value can extend or spoof it.
- **`FW Version`** stops **re-labelling**: without it, a genuinely signed v1.4.0 image
  could be re-announced as v9.9.9 — or as an *older* version, to walk a device backwards
  into a known-bad build — with the signature still checking out.
- **`Image Len`** stops a length lie: the announced length is what the installer reads
  and flashes, so it must be covered.
- `Image CRC-32` is deliberately **not** covered — it is a pure function of the image
  bytes, which *are* covered, so signing it would add nothing. `Sig Scheme` is not
  covered either: any rewritten scheme value only moves the container into a bucket the
  armer rejects outright (§1.4).

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

Steps 6 and 7 run over a **single streaming pass** of the image: the same bytes feed
the CRC and the signature hash, so verification adds no second read of the card and no
image-sized buffer. Corruption (6) is reported **before** trust (7) on purpose — a torn
copy is the likelier failure and "the file is damaged, copy it again" is the actionable
message, whereas telling a rider to re-copy an intact but forged image would be a lie.

The bootloader performs **no signature check** (§1.2). Its guarantee is unchanged and
independent: verify-before-erase by CRC over the raw extents, and always leave a
bootable image. Signature verification is an *authorization* gate on arming, not a
second integrity gate on flashing.

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

`Generation` is a diagnostic breadcrumb, **not** a replay guard. It is bumped on every
arm, carried for `Armed`/`Trial` (read back by `BootState::generation()`), and recorded
inside the `Idle` payload's **Last Outcome** record (§2.2). Its one live consumer is the
app's boot-outcome reconcile, which matches the recorded generation against the arm
marker it left behind to tie an outcome to *its* arm (§2.2). Nothing compares generations
to *reject* a page: the single-page overwrite-in-place channel has no live stale-replay
vector, and `Idle` pins the header field to `0`, so the counter is not monotonic across a
cycle (`Idle 0 → Armed 1 → Idle 0`). A torn, blank, or stale page is caught by the CRC
frame and decodes to `Idle` (invariant 4 in §Design principles) regardless of generation.

**Format Version history.** `0x0001` was the original layout. `0x0002` (DR2 #730)
appended the **Last Outcome** record to the Idle payload so the bootloader's terminal
writes carry *what happened* — a rollback vs. a pre-erase reject are otherwise
indistinguishable when the running and staged images share a version string
(a same-version re-stage), which made every such failure misreport as a success.

**Version compatibility (normative).** Readers **MUST accept both** `0x0001` and
`0x0002`; writers **MUST emit** `0x0002`. The `Armed` and `Trial` payload layouts are
byte-identical across the two versions; a `0x0001` `Idle` body simply ends after the
installed option (§2.2) and decodes with no Last Outcome — the decoder gates this on
the version field and MUST NOT parse the zero padding after a v1 payload as an
outcome record. Read-compatibility is load-bearing, not a courtesy: the bootloader is
flashed once by probe and is **not** updated by DFU, so an already-fielded bootloader
keeps writing `0x0001` pages after the app updates. The skew matrix:

| Bootloader | App | Behavior |
| :-- | :-- | :-- |
| v1 | v1 | The original protocol, unchanged |
| v1 | v2 | **Works, including the trial confirm**: the app reads the bootloader's freshly-written v1 `Trial` (byte-identical) and confirms it, so the update that crosses this bump installs and sticks. The v1 `Idle` the bootloader writes on a failure carries no outcome record, so the verdict card for that boot falls back to the conservative failure verdict |
| v2 | v1 | Symmetric by the same rule (both sides read v1 and v2); only occurs transiently on a dev bench (a reflashed bootloader under an old app) |
| v1 bootloader reading a v2 page | — | The one degraded case: the new app arms a **subsequent** update, the old bootloader cannot decode the v2 `Armed`, falls back to `Idle`, and jumps — the install never starts and the app's verdict honestly reports the not-started failure card. Recoverable (reflash the bootloader), never a revert loop |

The practical consequence: the app update that carries this bump installs and sticks
on an old bootloader; only *further* DFU updates require the bootloader reflash.

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
the arm and booted the intact old app (DR3 #731 — see §2.4). `Outcome Generation` lets the app's
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

**`MAX_EXTENTS` = 96.** A ~900 KB image over 16 KB FAT clusters resolves to ≈56
extents; 96 leaves headroom for a moderately fragmented card. The armer errors out
past this (suggesting a re-copy to defragment) rather than truncating the chain, and
the decoder rejects an Extent Count above `MAX_EXTENTS`.

`Len` and `Image CRC-32` deliberately duplicate the embedded header's `Image Len`
and `Image CRC-32` (so the installer reads them without re-decoding the header) and
**MUST match** them; decoders MUST reject a `StagedRef` where either pair disagrees
(a diverging record was never built from one coherent image).

**What the extents cover.** The chain locates the **whole staged file**: the armer
resolves `UPDATE.BIN` as-is, so the chain's byte stream begins with the file's own
64-byte OBCU header (§1.1) followed by the raw image; everything past `64 + Len` — the
v2 signature trailer (§1.3) and then FAT cluster slack — is ignored by the installer.
`Len` / `Image CRC-32` remain **raw-image** values: the installer's verify pass reads
the leading 64 bytes only to check they decode to exactly the `Header` recorded above,
then CRCs the next `Len` bytes — and its flash pass writes those same `Len` bytes (the
container header is skipped, never flashed; the trailer is never even read) to the app
slot. This is normative for both the armer (S4) and the bootloader; the skip arithmetic
lives once, in `obc-dfu`'s install engine, and is unchanged by v2.

**The rollback snapshot is unsigned.** `ROLLBACK.BIN` — the armer's copy of the running
image, written from the app slot before an install — is a **v1/unsigned** container
(`Sig Scheme` = `0`). The device cannot reconstruct the original release signature from
slot bytes alone, and nothing needs one: the snapshot never passes through the armer's
scan (§1.4), and the bootloader's rollback path validates it by CRC like everything
else. Marking it signed with no trailer behind it would make the file lie to
`obc-mkimage inspect`. The `StagedRef` the armer records for the snapshot carries the
same unsigned header, so the installer's header-equality check still matches.

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
bootloader starts the dog itself — with the app's exact config; the shared 24 s
period is `obc_dfu::WDT_TIMEOUT_TICKS` — immediately before the trial jump, so the
guarantee holds even on a cold power-on where no watchdog was running yet. On the
warm-reset arm path the app's already-running dog is instead adopted and fed
through the install, so a slow install is never cut down mid-flash; a plain `Idle`
boot never touches the watchdog (DR1, #729).

**Unreadable-card handling (DR3, #731).** A card the bootloader cannot read is retried
with a growing backoff, but *how long* depends on whether the app slot has been touched —
the same verify-before-erase line that governs everything else. **Before** the engine's
flash pass begins (a bring-up failure, or an SD error during the verify pass of an
`Armed` install) the old app is still intact, so after a bounded budget of pre-erase
failures (~a minute) the bootloader **abandons** the arm: it writes `Idle` — carrying the
outgoing image's header forward exactly as a rejected stage does — with an `ArmAbandoned`
Last Outcome, and boots the intact old app. **Once the flash pass has begun** (the slot may
be half-written) and for a `Rollback` (whose trial image is the only bootable thing), an SD
error instead retries **forever** (the "reinsert the card and power-cycle" worst case) — a
touched slot is never abandoned. The retry *count* is a bootloader policy; the "abandon
writes `Idle` + `ArmAbandoned`, pre-erase only" rule is host-tested in `obc-dfu`
(`engine::abandon_arm`). This refines epic #615's invariant 5 ("card absent ⇒ retry every
boot"): that still holds for the erase-unsafe cases, but a pre-erase `Armed` arm no longer
strands a device that holds perfectly good firmware.

---

## 3. Storage-blob stage carve

Since the storage pivot (epic #1158) the microSD card is only reachable through Nordic's
**sEMMC soft peripheral** — a position-independent RISC-V image the FLPR coprocessor
executes. The app embeds that image in its own flash; the 32 KB bootloader cannot
(image + driver + engine overflow its carve), and it must not read it out of the app
slot, because the install engine rewrites the slot **while still streaming the staged
image from the card** — a power cut mid-flash could then destroy the only reachable
copy of the thing needed to finish the install. The armer therefore **stages the blob
into a dedicated RRAM carve** the bootloader reads instead.

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

The addresses live only in the linker scripts (`__semmc_stage_base`, the
`__boot_state_base` convention): the board crate's `build.rs` emits the `SEMMC_STAGE`
region and sizes it from the shared constant; `obc-boot`'s static `memory.x` mirrors it.
The carve length matches the RAM carve the image executes in (`SEMMC_CARVE_BYTES`), so a
grown future blob never forces a second layout change.

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
checks, and that the declared image fits the execution carve. The *platform* half of the
id word is deliberately **not** pinned: the bootloader is flashed once, and a future
blob revision for a newer platform of the same peripheral must remain usable. The VRI
offset is taken from the validated metadata, never hard-coded.

An `Armed` decision whose carve fails validation is **abandoned** like an unreadable
card past its retry budget (§2.4's DR3 path — the slot is untouched); a `Rollback`
decision whose carve fails validation parks (SOS) rather than guessing — unreachable
from the §3.3 ordering short of RRAM decay or SWD interference, and a power cycle
retries.

---

## Reference implementation

`firmware/obc-dfu` (`no_std`, `core`-only apart from the Ed25519 verifier): `image.rs`
(`ImageHeader`, `MAX_IMAGE_LEN`, `MAX_CONTAINER_LEN`, the vector-table SP check),
`state.rs` (`BootState`, `StagedRef`, `Extent`, `MAX_EXTENTS`, `decide`, the
16-byte-line-aligned page codec), `crc32.rs` (the canonical DFU-side CRC-32/IEEE),
`engine.rs` (the bootloader's install engine — the verify → flash → readback →
state-transition sequencing over a small `InstallIo` trait, host-tested with mock IO in
`tests/engine.rs`, including the §2.3 header-skip arithmetic), `blobstage.rs` (§3: the
stage frame codec and the soft-peripheral metadata validation, host-tested in
`tests/blobstage.rs` including against the vendored image), `sig.rs` (§1.3: the
`signing_prefix` message layout — the single definition both the host signer and the
device verifier go through — the embedded `RELEASE_PUBKEY`, and a **streaming**
`Verifier` so §1.4's steps 6–7 share one pass), and `armer.rs` (§1.4's ordered
acceptance matrix; the trusted key is a **parameter**, never a build flag, so the tests
exercise the shipping path with a test key).

The Ed25519 implementation is [`ed25519-compact`](https://crates.io/crates/ed25519-compact)
with `default-features = false, features = ["opt_size"]`: `core`-only, no transitive
dependencies, and the only lean choice that offers *incremental* verification. `obc-boot`
links `obc-dfu` but never calls into `sig`, so the crate is dropped entirely from the
32 KB bootloader image (zero `ed25519_compact` symbols in its ELF).

The host tool `host/obc-mkimage` generates keys, produces and signs §1 containers, and
verifies them (`keygen` / `wrap [--sign-seed]` / `sign` / `inspect`); `inspect` exits
non-zero on any failure, which is what makes it the release pipeline's gate.
Format-contract tests build blobs by hand and round-trip every variant
(`obc-dfu/tests/boot_state.rs`, `obc-dfu/tests/signature.rs` — the §1.2 compatibility
proof and §1.4's reject matrix — `obc-dfu/tests/vectors.rs` for the shared fixture pair,
the unit tests in `image.rs` and `sig.rs`, and `obc-mkimage/tests/cli.rs`); see
`firmware/README.md` for the `objcopy → wrap → sign` pipeline.
