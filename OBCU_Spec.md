# OBCU File Format Specification (v1)

OBCU (OpenBikeComputer Update) is the byte format of a **field firmware update** —
the SD-staged DFU foundation (epic #615). It has two parts, both defined here and
both implemented by the shared `no_std` crate `firmware/obc-dfu` (host-tested to the
same bar as the settings codec, and linked by the `obc-boot` bootloader,
`firmware/obc-boot`):

1. **The update-image container** (§1) — the `UPDATE.BIN` file on the SD card: a
   fixed **64-byte header** followed by the raw application image.
2. **The boot-state page** (§2) — the CRC-framed blob in a dedicated **4 KB RRAM
   page**, the sole handoff channel between the app (the *armer*) and the bootloader
   (the *installer*).

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
   reads raw SPI blocks with no filesystem in the 32 KB bootloader budget.
4. **CRC-32/IEEE only, no signatures in v1** (locked). Physical card access is
   already root on an open device. The header reserves space (§1) for a future
   signature-scheme marker if internet-sourced OTA ever lands.

All multi-byte integers are **little-endian**. The integrity check everywhere is
**CRC-32/IEEE** (reflected polynomial `0xEDB88320`, init/xor-out `0xFFFFFFFF`, check
value `crc32("123456789") == 0xCBF43926`).

---

## 1. Update-image container

```
[OBCU header]   (64 bytes, fixed)
[raw image]     (image_len bytes — the app's objcopy -O binary output, vector table first)
```

The file lives at the **card root** as `UPDATE.BIN` (8.3-safe, locked). It is
produced by the `obc-mkimage wrap` host tool and consumed by the app-side armer,
which validates the header + full image CRC before staging.

### 1.1 Header (64 bytes)

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 0 | Magic | 4 | `char[4]` | Must be `b"OBCU"` |
| 4 | Header Version | 2 | `uint16` | `0x0001` (readers reject any other value) |
| 6 | Reserved | 2 | — | `0` |
| 8 | Image Len | 4 | `uint32` | Bytes of the raw image following the header |
| 12 | Image CRC-32 | 4 | `uint32` | CRC-32/IEEE over the raw image **only** |
| 16 | FW Version | 32 | `char[32]` | UTF-8 `git describe` string, NUL-padded |
| 48 | Reserved | 12 | — | `0` — space for a future signature-scheme marker |
| 60 | Header CRC-32 | 4 | `uint32` | CRC-32/IEEE over header bytes `0..60` |

**Decode rule** (`ImageHeader::decode(&[u8; 64]) -> Option`): return `None` on bad
magic, a Header Version other than `1`, or a Header CRC-32 that doesn't match bytes
`0..60`; otherwise `Some`. This is the settings-store convention — a **valid CRC ⇒
`Some`**, and a version change is a hard reject, never a silent migration. The
raw-image CRC (offset 12) is verified **separately**, against the staged image
bytes, by whoever is about to trust them (the armer over the file, the bootloader
over the resolved extents).

`FW Version` is read back with trailing NULs trimmed; an over-long version string is
truncated to 32 bytes on a UTF-8 char boundary at wrap time (never mid-codepoint).

`Image Len` must not exceed **`MAX_IMAGE_LEN` = 1,480,000** bytes — the L15 DK app
slot (`0x8000 … 0x17B000`) minus a small margin. `obc-mkimage wrap` refuses a larger
image. (The LM20's larger slot is a future mechanical bump.)

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
| 4 | Format Version | 2 | `uint16` | `0x0001` |
| 6 | State Tag | 1 | `uint8` | `0` Idle · `1` Armed · `2` Trial |
| 7 | Reserved | 1 | — | `0` |
| 8 | Blob Len | 4 | `uint32` | Total encoded length, incl. CRC; a multiple of 16 |
| 12 | Generation | 4 | `uint32` | Bumped on every arm; `0` for Idle |

`Generation` lets the installer reject a stale-page replay (invariant 4): it is
carried for `Armed`/`Trial` and read back by `BootState::generation()`.

### 2.2 Payload by State Tag

**Tag `0` — Idle** (`installed: Option<ImageHeader>`): the header of the running
image, for the UI and to seed a rollback snapshot.

| Offset | Field | Size | Type | Description |
| :-- | :-- | :-- | :-- | :-- |
| 16 | Has Installed | 1 | `uint8` | `0` none · `1` a header follows |
| 17 | Installed Header | 64 | `ImageHeader` | Present only when Has Installed = 1 (§1.1) |

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
64-byte OBCU header (§1.1) followed by the raw image; any tail of the final block
past `64 + Len` is FAT cluster slack and is ignored. `Len` / `Image CRC-32` remain
**raw-image** values: the installer's verify pass reads the leading 64 bytes only to
check they decode to exactly the `Header` recorded above, then CRCs the next `Len`
bytes — and its flash pass writes those same `Len` bytes (the container header is
skipped, never flashed) to the app slot. This is normative for both the armer (S4)
and the bootloader; the skip arithmetic lives once, in `obc-dfu`'s install engine.

### 2.4 Decode rule and boot decision

**`BootState::decode(&[u8]) -> BootState`** returns `Idle { installed: None }` for
**anything** but a clean read of this format: too short, bad magic, a Format Version
other than `1`, a `Blob Len` that is out of range or not a multiple of 16, a failed
whole-blob CRC, an unknown State Tag, an Extent Count over `MAX_EXTENTS`, a
`StagedRef` whose redundant `Len`/`Image CRC-32` disagree with its embedded header
(§2.3), or a nested `ImageHeader` whose own CRC fails. This is the torn-write safety
net — the bootloader always receives a sane state.

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
hardware watchdog guarantees a wedged trial boot becomes the next boot.

---

## Reference implementation

`firmware/obc-dfu` (`no_std`, `core`-only): `image.rs` (`ImageHeader`,
`MAX_IMAGE_LEN`, the vector-table SP check), `state.rs` (`BootState`, `StagedRef`,
`Extent`, `MAX_EXTENTS`, `decide`, the 16-byte-line-aligned page codec), `crc32.rs`
(the canonical DFU-side CRC-32/IEEE), `engine.rs` (the bootloader's install engine —
the verify → flash → readback → state-transition sequencing over a small `InstallIo`
trait, host-tested with mock IO in `tests/engine.rs`, including the §2.3 header-skip
arithmetic). The host tool `firmware/obc-mkimage` produces
and inspects §1 containers (`wrap` / `inspect`). Format-contract tests build blobs by
hand and round-trip every variant (`obc-dfu/tests/boot_state.rs`, the unit tests in
`image.rs`, `obc-mkimage/tests/cli.rs`); see `firmware/README.md` for the
`objcopy → wrap` pipeline.
