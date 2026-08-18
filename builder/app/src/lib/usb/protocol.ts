/**
 * The control-plane codecs of [`obc-ble-interface-spec.md`](../../../../../specs/obc-ble-interface-spec.md),
 * in TypeScript: the transfer descriptor (§4.2), the `status` envelope (§4.3), the `command` writes
 * (§4.4), the identity read (§1) and the Config blob (§7.3).
 *
 * These are ports of `firmware/obc-ble/src/descriptor.rs`, field for field, and they are pinned to
 * the **same** `specs/vectors/` fixtures the firmware's `cargo test -p obc-vectors` and the iOS
 * app's `swift test` assert against (see `vectors.test.ts`). Three implementations, one set of
 * bytes: a divergence here is a bug here, never a reason to move a fixture.
 *
 * USB is a second *transport*, not a second protocol — everything in this file is the wire the BLE
 * link already carries. What USB adds is a frame saying which of BLE's characteristics a control
 * message belongs to; its encoding lives in `transport.ts` and is provisional until #889 settles the
 * device side. **Nothing in this file moves when that is decided** — which is the whole point.
 *
 * All integers are little-endian, matching OBCM/OBCR.
 */

/** The wire version this client speaks. A device answering anything else is refused (§1). */
export const PROTOCOL_VERSION = 2;

/** Why a control-plane decode failed. Mirrors `DescriptorError` in `descriptor.rs`. */
export type DecodeErrorCode = "truncated" | "unknown-op" | "unknown-type" | "unknown-status";

/** A malformed or unrecognised control message. */
export class DecodeError extends Error {
    readonly code: DecodeErrorCode;

    constructor(code: DecodeErrorCode, message: string) {
        super(message);
        this.name = "DecodeError";
        this.code = code;
    }
}

// --- object types and ops (§4.1, §4.2) ---------------------------------------

/** The kind of object a bulk transfer carries (§4.1). */
export const ObjectType = {
    Route: 1,
    Ride: 2,
    /** Reserved on the bulk channel — Config crosses the control channel whole-blob. */
    ConfigBlob: 3,
    Diagnostics: 4,
    /** A complete `UPDATE.BIN` OBCU container. The transfer layer stays format-blind (§7.6). */
    FwImage: 5,
    RouteList: 6,
    RideList: 7,
    /** Dev/test loopback: the device streams back exactly what it received. */
    Echo: 8,
    Trip: 9,
    TripList: 10,
    /**
     * An OBCM map file, written to the card verbatim.
     *
     * **Provisional — #889 ratifies this number.** The interface spec's object table stops at `10`
     * and reserves `11`–`15` for sensors (M4), so this takes the first value past that band rather
     * than the first free one: a map arriving where an M4 sensor object was expected is a worse
     * failure than a gap in the numbering.
     *
     * It has to exist for C4 (#903) at all — a map is the one thing the object model never carried,
     * because a 200 MB file over BLE was never on the table and USB is what changes that. The
     * transfer semantics are the ones every other object already has (announce, stream, whole-object
     * CRC, commit), and a map is uploaded as {@link NEW_OBJECT_ID} like a route; a re-upload of an
     * artifact the device already holds dedups on (length, CRC) exactly as §4.1 specifies. What is
     * *not* settled here, because C4 does not need it, is how maps are enumerated and named on the
     * card — there is no `mapList` and no naming field, and #889 (or the device dashboard, D-phase)
     * owns both.
     */
    Map: 16,
    /** One OBCM shard in a manifest-committed volume set (USB only). */
    MapShard: 17,
    /** The OBCA manifest that commits a previously uploaded shard set (USB only). */
    MapSet: 18,
    /**
     * The set's **terrain shard** — an OBCT raster, `MS<id>.OBD` on the card (USB only, #1044).
     *
     * It is not a {@link MapShard}, and the difference is load-bearing rather than cosmetic. A
     * shard's `object_id` is a packed `(shard_count, index)` naming one of the OBCM files the
     * manifest's *leading* records describe; a raster has no index, is not an OBCM file, and lands
     * under a different name. Sent as a shard it would consume an index the manifest never names.
     *
     * New-only (`object_id` = {@link NEW_OBJECT_ID}): there is at most one per set. It goes out
     * **after every `mapShard` and before the `mapSet`**, because `OBCA_Spec.md` §5.2's `Shard
     * Count` counts every record — terrain included — so the manifest is 56 bytes longer than the
     * shard count alone implies and the device checks that length at the announce.
     */
    TerrainShard: 19,
} as const;
export type ObjectType = (typeof ObjectType)[keyof typeof ObjectType];

const OBJECT_TYPES: ReadonlySet<number> = new Set(Object.values(ObjectType));

/** The imperative a descriptor carries (§4.2). */
export const Op = { Upload: 1, Download: 2, Abort: 3 } as const;
export type Op = (typeof Op)[keyof typeof Op];

/** The `object_id` an upload sends to mean "new — the device assigns the id" (§4.1). */
export const NEW_OBJECT_ID = 0xffff;

/** Object id of the singletons: the list objects, diagnostics, and the `fwImage` staging slot. */
export const SINGLETON_OBJECT_ID = 0;

/** The OBCT terrain container's fixed header width (`OBCT_Spec.md` §4) — the floor a
 *  {@link ObjectType.TerrainShard} announce has to clear, as an OBCM header is for a map. */
export const OBCT_HEADER_LEN = 32;
/** `b"OBCT"` — the terrain container's magic, and the version this firmware reads. */
export const OBCT_MAGIC = "OBCT";
export const OBCT_VERSION = 1;

/**
 * The volume-set corner of this codec — `OBCS_*`, {@link manifestLen}, {@link setPartId} and the
 * {@link ObjectType.MapShard} / {@link ObjectType.MapSet} / {@link ObjectType.TerrainShard} kinds.
 *
 * **Nothing in this app sends one any more, and they stay anyway.** A map is one OBCM object since
 * OBCM v14 (#1420), so the flows that spoke this are deleted — but the *firmware* still accepts
 * these kinds from a card written before the cut, and this file is the transcription of the wire
 * contract rather than of any particular flow. It is exactly the boundary `obc_formats::obcs` sits
 * on for the same reason and until the same slice.
 *
 * What keeps it honest is `vectors.test.ts`, which round-trips `specs/vectors/transfer-set-*.bin` —
 * the same three checked-in vectors `obc-vectors` and `obc-ble`'s own tests decode on the Rust side.
 * So this is a codec under test against shared bytes, not stranded code with no caller.
 *
 * It goes when the board cutover retires the kinds (FS7.5c), together with the vectors.
 */
/** The OBCS set manifest's fixed header width (`OBCA_Spec.md` §5.2). Unmoved by v3. */
export const OBCS_HEADER_LEN = 72;
/** The OBCS manifest's fixed per-record width (§5.2) — `64` since v3 appended the member id. */
export const OBCS_RECORD_LEN = 64;
/** The one manifest version a reader accepts — `3` since every record carries its member's
 *  `ObjectId` (§5.2, #1389). A hard cut: v2's records are 56 bytes, so a reader that guessed would
 *  find every record but the first at the wrong offset. */
export const OBCS_VERSION = 3;
/** Offset of the member `ObjectId` inside a record (§5.2). */
export const OBCS_MEMBER_ID_OFFSET = 56;

/**
 * The exact byte length of a set manifest carrying `records` records — `72 + 64 × Shard Count`.
 *
 * **`records` is not the shard count** (#1044). §5.2's `Shard Count` field counts *every* record,
 * and a set with elevation carries one more: the `terrain` role's. A device refuses a `mapSet`
 * announce whose `total_len` is anything else, before a byte streams — so a host that sends every
 * OBCM shard and skips the raster announces a manifest one record longer than the device can expect,
 * and loses the whole upload at its last transfer.
 */
export function manifestLen(records: number): number {
    return OBCS_HEADER_LEN + OBCS_RECORD_LEN * records;
}

/** Pack a volume-set shard's `(shard_count, index)` into the descriptor's object id (§4.2). */
export function setPartId(shardCount: number, index: number): number {
    if (!Number.isInteger(shardCount) || shardCount < 1 || shardCount > 32) {
        throw new RangeError(`a map set must contain 1–32 shards, got ${shardCount}.`);
    }
    if (!Number.isInteger(index) || index < 0 || index >= shardCount) {
        throw new RangeError(`shard index ${index} is outside a ${shardCount}-shard set.`);
    }
    return (shardCount << 8) | index;
}

/**
 * The fixed **12-byte** transfer descriptor (§4.2) — one shape for upload, download request,
 * download announce and abort, which is what lets the bulk channel carry payload and nothing else.
 *
 * ```text
 *   op         u8    1 = upload · 2 = download · 3 = abort
 *   type       u8    ObjectType
 *   object_id  u16   0xFFFF on upload = "new"
 *   total_len  u32   upload / download announce: full object size · request / abort: 0
 *   crc32      u32   upload / download announce: whole-object CRC-32 · request / abort: 0
 * ```
 *
 * Protocol v2 dropped v1's trailing, permanently-zero `offset`: transfers restart, they never
 * resume (§1 principle 4).
 */
export interface TransferControl {
    op: Op;
    type: ObjectType;
    objectId: number;
    totalLen: number;
    crc32: number;
}

export const TRANSFER_CONTROL_LEN = 12;

export function encodeTransferControl(d: TransferControl): Uint8Array {
    const out = new Uint8Array(TRANSFER_CONTROL_LEN);
    const view = new DataView(out.buffer);
    out[0] = d.op;
    out[1] = d.type;
    view.setUint16(2, d.objectId, true);
    view.setUint32(4, d.totalLen, true);
    view.setUint32(8, d.crc32, true);
    return out;
}

export function decodeTransferControl(data: Uint8Array): TransferControl {
    need(data, TRANSFER_CONTROL_LEN, "transfer descriptor");
    const view = viewOf(data);
    return {
        op: op(data[0]),
        type: objectType(data[1]),
        objectId: view.getUint16(2, true),
        totalLen: view.getUint32(4, true),
        crc32: view.getUint32(8, true),
    };
}

// --- the `status` envelope (§4.3) --------------------------------------------

/** The outcome of a transfer (§4.3 `msg = 1`). */
export const TransferStatus = {
    Committed: 0,
    CrcMismatch: 1,
    Aborted: 2,
    Error: 3,
    NotFound: 4,
    Busy: 5,
    StorageFull: 6,
} as const;
export type TransferStatus = (typeof TransferStatus)[keyof typeof TransferStatus];

const TRANSFER_STATUSES: ReadonlySet<number> = new Set(Object.values(TransferStatus));

/** Human-readable names for the status codes, for error messages the rider will read. */
export const TRANSFER_STATUS_NAMES: Readonly<Record<TransferStatus, string>> = {
    [TransferStatus.Committed]: "committed",
    [TransferStatus.CrcMismatch]: "crcMismatch",
    [TransferStatus.Aborted]: "aborted",
    [TransferStatus.Error]: "error",
    [TransferStatus.NotFound]: "notFound",
    [TransferStatus.Busy]: "busy",
    [TransferStatus.StorageFull]: "storageFull",
};

/** The result of a `command` write (§4.3 `msg = 3`). */
export const CommandStatus = { Ok: 0, UnknownCommand: 1, NotFound: 2, Busy: 3, Error: 4 } as const;
export type CommandStatus = (typeof CommandStatus)[keyof typeof CommandStatus];

const COMMAND_STATUSES: ReadonlySet<number> = new Set(Object.values(CommandStatus));

export const COMMAND_STATUS_NAMES: Readonly<Record<CommandStatus, string>> = {
    [CommandStatus.Ok]: "ok",
    [CommandStatus.UnknownCommand]: "unknown command",
    [CommandStatus.NotFound]: "not found",
    [CommandStatus.Busy]: "busy",
    [CommandStatus.Error]: "error",
};

/**
 * One `status` notification: a `u8` discriminator plus a fixed body. In protocol v2 this is the
 * **sole** device → host control channel — one ordering domain for transfer results, store-change
 * edges, command results and download announces alike.
 */
export type StatusMessage =
    | { msg: "transferResult"; objectId: number; status: TransferStatus; committedOffset: number }
    | { msg: "storeChanged"; type: ObjectType; revision: number }
    | { msg: "commandResult"; command: number; status: CommandStatus; detail: number }
    | { msg: "downloadAnnounce"; descriptor: TransferControl };

export function encodeStatusMessage(m: StatusMessage): Uint8Array {
    switch (m.msg) {
        case "transferResult": {
            const out = new Uint8Array(8);
            const view = new DataView(out.buffer);
            out[0] = 1;
            view.setUint16(1, m.objectId, true);
            out[3] = m.status;
            view.setUint32(4, m.committedOffset, true);
            return out;
        }
        case "storeChanged": {
            const out = new Uint8Array(6);
            out[0] = 2;
            out[1] = m.type;
            new DataView(out.buffer).setUint32(2, m.revision, true);
            return out;
        }
        case "commandResult":
            return new Uint8Array([3, m.command, m.status, m.detail]);
        case "downloadAnnounce": {
            const out = new Uint8Array(1 + TRANSFER_CONTROL_LEN);
            out[0] = 4;
            out.set(encodeTransferControl(m.descriptor), 1);
            return out;
        }
    }
}

/**
 * Decode a `status` notification.
 *
 * Returns `null` for an **unknown discriminator**, which the spec requires readers to ignore
 * rather than fail on — that is the forward-compatibility hinge that lets a later firmware add a
 * message type without breaking a shipped browser tab. Throws only when a *known* discriminator
 * carries a malformed body.
 */
export function decodeStatusMessage(data: Uint8Array): StatusMessage | null {
    if (data.length < 1) throw new DecodeError("truncated", "an empty status notification arrived.");
    const view = viewOf(data);
    switch (data[0]) {
        case 1:
            need(data, 8, "transferResult");
            return {
                msg: "transferResult",
                objectId: view.getUint16(1, true),
                status: transferStatus(data[3]),
                committedOffset: view.getUint32(4, true),
            };
        case 2:
            need(data, 6, "storeChanged");
            return { msg: "storeChanged", type: objectType(data[1]), revision: view.getUint32(2, true) };
        case 3:
            need(data, 4, "commandResult");
            return { msg: "commandResult", command: data[1], status: commandStatus(data[2]), detail: data[3] };
        case 4:
            need(data, 1 + TRANSFER_CONTROL_LEN, "downloadAnnounce");
            return { msg: "downloadAnnounce", descriptor: decodeTransferControl(data.subarray(1)) };
        default:
            return null;
    }
}

// --- `command` writes (§4.4) --------------------------------------------------

/** The command bytes (§4.4). Next free is `7`. */
export const Command = {
    DeleteObject: 1,
    AckRides: 2,
    InstallFw: 3,
    ForgetBond: 4,
    SetClock: 5,
    SetRouteRetention: 6,
} as const;
export type Command = (typeof Command)[keyof typeof Command];

/** `deleteObject` (cmd 1): `type u8 · object_id u16`. Routes and trips only — a ride delete is
 *  reserved and answered `notFound` (rides are deleted on the device itself). */
export function encodeDeleteObject(type: ObjectType, objectId: number): Uint8Array {
    const out = new Uint8Array(4);
    out[0] = Command.DeleteObject;
    out[1] = type;
    new DataView(out.buffer).setUint16(2, objectId, true);
    return out;
}

/**
 * `ackRides` (cmd 2): `count u8 · count × object_id u16` — the possession ack.
 *
 * **The hosted web tier never sends this.** #894 locks ride-sync semantics: `synced` means "a
 * durable copy exists off the device", and a browser download the user may cancel keeps no record,
 * so acking from a tab would start an expiry countdown against a ride nobody holds. The encoder
 * lives here because the desktop app (E1 #911) does ack, after fsync, over this same client — C5
 * (#904) must not call it.
 */
export function encodeAckRides(rideIds: readonly number[]): Uint8Array {
    if (rideIds.length > 0xff) throw new RangeError(`ackRides carries at most 255 ids, got ${rideIds.length}`);
    const out = new Uint8Array(2 + rideIds.length * 2);
    const view = new DataView(out.buffer);
    out[0] = Command.AckRides;
    out[1] = rideIds.length;
    rideIds.forEach((id, i) => view.setUint16(2 + i * 2, id, true));
    return out;
}

/** `installFw` (cmd 3): the command byte alone. Requests the on-glass confirm flow; the device
 *  never installs without a physical Select press, so this returning `ok` means "asked", not
 *  "installed" (§4.4). */
export function encodeInstallFw(): Uint8Array {
    return new Uint8Array([Command.InstallFw]);
}

/** `forgetBond` (cmd 4): the command byte alone. BLE-only in practice — a USB host has no bond to
 *  dissolve — but the vocabulary is one protocol, so the encoder is here for completeness. */
export function encodeForgetBond(): Uint8Array {
    return new Uint8Array([Command.ForgetBond]);
}

/** The earliest UTC `setClock` accepts: 2020-01-01. An earlier stamp is an obviously-bogus host
 *  clock and the device answers `error` (§4.4). */
export const SET_CLOCK_MIN_UTC = 1_577_836_800;
/** The magnitude bound on a `setClock` offset: ±14 h, the real-world span. */
export const SET_CLOCK_MAX_OFFSET_MIN = 840;

/**
 * `setClock` (cmd 5): `utc u32 · offset_min i16` — the peer stamps the device's trusted wall clock.
 *
 * The device has no RTC, and a clock it merely resumed from flash is *untrusted*: nothing is
 * stamped or expired from it. Exactly two sources make it trusted for the boot — a GPS fix and this
 * command — which is why every connected peer sends it, browser included. The range checks are the
 * device's own, applied here so a bad value fails in the tab instead of costing a round trip.
 */
export function encodeSetClock(utcSeconds: number, offsetMinutes: number): Uint8Array {
    if (!Number.isInteger(utcSeconds) || utcSeconds < SET_CLOCK_MIN_UTC || utcSeconds > 0xffffffff) {
        throw new RangeError(`setClock utc ${utcSeconds} is outside [${SET_CLOCK_MIN_UTC}, 2^32).`);
    }
    if (!Number.isInteger(offsetMinutes) || Math.abs(offsetMinutes) > SET_CLOCK_MAX_OFFSET_MIN) {
        throw new RangeError(`setClock offset ${offsetMinutes} min is beyond ±${SET_CLOCK_MAX_OFFSET_MIN}.`);
    }
    const out = new Uint8Array(7);
    const view = new DataView(out.buffer);
    out[0] = Command.SetClock;
    view.setUint32(1, utcSeconds, true);
    view.setInt16(5, offsetMinutes, true);
    return out;
}

/** The largest valid retention level: `5` (2 months). Above it the device answers `error`. */
export const MAX_RETENTION = 5;

/** `setRouteRetention` (cmd 6): `object_id u16 · retention u8` — set a stored route's expiry
 *  policy without re-uploading it. Never touches the route's `last_used`, so a route mid-countdown
 *  keeps its anchor (§4.4). */
export function encodeSetRouteRetention(objectId: number, retention: number): Uint8Array {
    if (!Number.isInteger(retention) || retention < 0 || retention > MAX_RETENTION) {
        throw new RangeError(`retention ${retention} is outside 0..=${MAX_RETENTION}.`);
    }
    const out = new Uint8Array(4);
    out[0] = Command.SetRouteRetention;
    new DataView(out.buffer).setUint16(1, objectId, true);
    out[3] = retention;
    return out;
}

// --- the identity read (§1) ---------------------------------------------------

/**
 * The `protocolVersion` read: `version u16 · store_epoch u32 · obcm_version u8`, served at one of
 * **three** lengths. The length is the version mechanism — it has been since the 2-byte no-store
 * read (#776), and the `obcm_version` byte (#911) joins the same scheme rather than inventing one:
 *
 * | Bytes | Means |
 * | --: | :-- |
 * | 7 | the full read |
 * | 6 | a firmware predating `obcm_version` → `obcmVersion: null` |
 * | 2 | no mounted card → no era to name, and no room for the byte after it |
 *
 * The epoch names the store's id era, and every durable link the peer keeps — a route's device id,
 * a ride's synced flag — is scoped to `(serial, epoch)` so an era change can never silently alias
 * months-old ids. An absent epoch is a *failed* identity read, not epoch `0` (a legal value): the
 * spec's ack fail-closed contract says a connection whose identity read failed reconciles nothing.
 *
 * `obcmVersion` is the OBCM **map-format** version this device's firmware reads — a different
 * number in a different sequence from `version`, which is the wire contract. It is the one fact
 * `OBCC_Spec.md` §10 turns on: don't offer an assembly the connected device cannot read. A
 * trailing field the read did not carry is `null`, **never** a fabricated `0`: `obcmVersion: 0`
 * would read as "this device supports OBCM v0" and refuse every real map, where `null` correctly
 * means "unknown" and takes §6(c)'s no-known-target-firmware branch (offer it, stating the
 * version).
 */
export interface VersionRead {
    version: number;
    /** `null` when the device serves the 2-byte short read — no card, so no era to name. */
    storeEpoch: number | null;
    /** `null` when the read carried no `obcm_version` byte: a firmware predating it, or the
     *  store-less short read (which stops before the epoch, let alone the byte after it). */
    obcmVersion: number | null;
    /** The optional capability word (§1, WX3). `null` when the read carried no such field — a
     *  firmware predating it — and never a fabricated `0`: both mean "no optional contracts", but
     *  only one of them is something the device actually said. */
    featureBits: number | null;
}

/** Bit 0 of {@link VersionRead.featureBits}: the device speaks the Weather Request contract (§11).
 *  Nothing in the browser builder acts on it — the weather path is the phone's — but the identity
 *  read is one wire shape across BLE and USB, and a mirror that cannot represent a field it will
 *  routinely receive is a mirror that quietly drops it. */
export const FEATURE_WEATHER = 1 << 0;

export function encodeVersionRead(v: VersionRead): Uint8Array {
    // The four lengths, in the one place that decides them. A trailing field without the one
    // before it has no encoding — the fields are positional — so it is dropped rather than
    // silently shifted into its predecessor's bytes.
    const length =
        v.storeEpoch === null ? 2 : v.obcmVersion === null ? 6 : v.featureBits === null ? 7 : 11;
    const out = new Uint8Array(length);
    const view = new DataView(out.buffer);
    view.setUint16(0, v.version, true);
    if (length >= 6) view.setUint32(2, v.storeEpoch as number, true);
    if (length >= 7) out[6] = v.obcmVersion as number;
    if (length >= 11) view.setUint32(7, v.featureBits as number, true);
    return out;
}

export function decodeVersionRead(data: Uint8Array): VersionRead {
    if (data.length < 2)
        throw new DecodeError("truncated", `identity read is ${data.length} bytes, expected 2, 6, 7 or 11.`);
    const view = viewOf(data);
    return {
        version: view.getUint16(0, true),
        storeEpoch: data.length >= 6 ? view.getUint32(2, true) : null,
        // Bytes past the last field we know are ignored, not rejected: that append-only rule is
        // what let this field land without a `PROTOCOL_VERSION` bump, and it has to keep holding
        // for whatever lands after it.
        obcmVersion: data.length >= 7 ? data[6] : null,
        // A *partial* capability word (8, 9 or 10 bytes) is absent, not the bytes that turned up:
        // three bytes of a u32 are a broken read, not a small feature set, and treating them as
        // data could claim a contract the device never announced.
        featureBits: data.length >= 11 ? view.getUint32(7, true) : null,
    };
}

// --- the Config object (§7.3) -------------------------------------------------

/** The name cap, matching the OBCR route-name field. */
export const CONFIG_MAX_NAME = 48;
/** The whole-blob cap that let Config live on a GATT characteristic in the first place. */
export const CONFIG_MAX_ENCODED = 128;

/**
 * The Config object: `name_len u16 · name · units u8 · weather_refresh u8`, append-only.
 *
 * The trailing refresh byte is decoded **raw and unvalidated** — §11.8 makes an unrecognised value
 * fatal in one direction only (a phone → device write), and this host is never that direction.
 *
 * Readers must ignore unknown trailing bytes and treat absent trailing fields as "device default" —
 * that rule *is* the version mechanism, so this decoder never rejects a longer blob from a newer
 * firmware.
 */
export interface DeviceConfig {
    /** The device name — writing Config with a changed name *is* the rename (§7.3, Delta 1). */
    name: string;
    /** `0 = metric · 1 = imperial`. */
    units: number;
    /** The weather refresh interval (§7.3, §11.8) **as the raw byte**: `0 = Off · 1 = 15 · 2 = 30 ·
     *  3 = 60 · 4 = 120` minutes. `null` when the blob carried no such byte, which on a read means
     *  **device default** — not `Off`. Round-tripping a Config read through a writer that collapsed
     *  the two would silently disable weather on a plain rename.
     *
     *  Raw, and deliberately **not** range-checked here — see {@link knownWeatherRefresh}. */
    weatherRefresh: number | null;
}

/** The §11.8 refresh enum, as the wire discriminants. */
export const WeatherRefresh = {
    off: 0,
    every15: 1,
    every30: 2,
    every60: 3,
    every120: 4,
} as const;
export type WeatherRefreshValue = (typeof WeatherRefresh)[keyof typeof WeatherRefresh];

/** The device default (epic #1185) — also what an *absent* field means on a **read**. */
export const WEATHER_REFRESH_DEFAULT: WeatherRefreshValue = WeatherRefresh.every30;

/** Minutes per interval; `null` for `off`, which has no interval rather than a zero one. */
export const WEATHER_REFRESH_MINUTES: Record<WeatherRefreshValue, number | null> = {
    [WeatherRefresh.off]: null,
    [WeatherRefresh.every15]: 15,
    [WeatherRefresh.every30]: 30,
    [WeatherRefresh.every60]: 60,
    [WeatherRefresh.every120]: 120,
};

/**
 * The refresh interval **as a reader sees it**: the value when it names an interval this build
 * knows, or `null` when the field was absent *or* names one it does not (§11.8's read direction).
 *
 * The tolerance is the point, and it is why {@link decodeConfig} does not range-check. This host is
 * always the *reading* side — it never has to adopt an interval — and an unrecognised value arriving
 * from a device is a **newer** device, not a broken one. Rejecting it would mean that appending a
 * fifth interval, an ordinary append to an append-only enum, broke every shipped reader. The strict
 * rule belongs to the one direction that must honour the value: a phone → device Config write, which
 * this host does not perform (see `client.writeConfig`, which sends what the caller supplies).
 *
 * `null` is its own state — specifically **not** `off` and **not** the default. Callers that need to
 * tell "absent" from "unrecognised" read the raw {@link DeviceConfig.weatherRefresh} beside this.
 */
export function knownWeatherRefresh(raw: number | null): WeatherRefreshValue | null {
    if (raw === null) return null;
    return (Object.values(WeatherRefresh) as number[]).includes(raw) ? (raw as WeatherRefreshValue) : null;
}

export function encodeConfig(c: DeviceConfig): Uint8Array {
    const name = new TextEncoder().encode(c.name);
    if (name.length > CONFIG_MAX_NAME) {
        throw new RangeError(`device name is ${name.length} UTF-8 bytes, the cap is ${CONFIG_MAX_NAME}.`);
    }
    const out = new Uint8Array(2 + name.length + 1 + (c.weatherRefresh === null ? 0 : 1));
    new DataView(out.buffer).setUint16(0, name.length, true);
    out.set(name, 2);
    out[2 + name.length] = c.units;
    if (c.weatherRefresh !== null) out[3 + name.length] = c.weatherRefresh;
    return out;
}

export function decodeConfig(data: Uint8Array): DeviceConfig {
    if (data.length < 3 || data.length > CONFIG_MAX_ENCODED) {
        throw new DecodeError("truncated", `Config blob is ${data.length} bytes, expected 3..=${CONFIG_MAX_ENCODED}.`);
    }
    const nameLen = viewOf(data).getUint16(0, true);
    if (nameLen > CONFIG_MAX_NAME || 2 + nameLen + 1 > data.length) {
        throw new DecodeError("truncated", `Config name_len ${nameLen} does not fit its ${data.length}-byte blob.`);
    }
    return {
        name: new TextDecoder().decode(data.subarray(2, 2 + nameLen)),
        units: data[2 + nameLen],
        weatherRefresh: data.length >= 4 + nameLen ? data[3 + nameLen] : null,
    };
}

// --- shared decode helpers ----------------------------------------------------

/** A `DataView` over exactly the bytes of `data` — `subarray` keeps the parent buffer, so the
 *  offset and length have to be carried explicitly or every field read is off by the slice. */
export function viewOf(data: Uint8Array): DataView {
    return new DataView(data.buffer, data.byteOffset, data.byteLength);
}

function need(data: Uint8Array, len: number, what: string): void {
    if (data.length < len) {
        throw new DecodeError("truncated", `${what} is ${data.length} bytes, the layout needs ${len}.`);
    }
}

function op(v: number): Op {
    if (v !== Op.Upload && v !== Op.Download && v !== Op.Abort) {
        throw new DecodeError("unknown-op", `transfer op ${v} is not upload (1), download (2) or abort (3).`);
    }
    return v;
}

export function objectType(v: number): ObjectType {
    if (!OBJECT_TYPES.has(v)) throw new DecodeError("unknown-type", `object type ${v} is unknown or reserved.`);
    return v as ObjectType;
}

function transferStatus(v: number): TransferStatus {
    if (!TRANSFER_STATUSES.has(v)) throw new DecodeError("unknown-status", `transfer status ${v} is unknown.`);
    return v as TransferStatus;
}

function commandStatus(v: number): CommandStatus {
    if (!COMMAND_STATUSES.has(v)) throw new DecodeError("unknown-status", `command status ${v} is unknown.`);
    return v as CommandStatus;
}
