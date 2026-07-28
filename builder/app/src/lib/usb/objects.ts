/**
 * The bulk-channel object layouts of the interface spec: the list objects (§7.4), the ride object
 * (§7.2) and the trip object (§7.7).
 *
 * A port of `firmware/obc-ble/src/list.rs` and the ride/trip layouts, pinned to the same
 * `specs/vectors/` fixtures (see `vectors.test.ts`). Routes and firmware images are absent on
 * purpose: an `.obcr` route and an `UPDATE.BIN` cross the wire as *opaque bytes* the device writes
 * verbatim (§7.1, §7.6), so the transfer layer stays format-blind and there is nothing here to
 * decode. The browser's OBCR encoding is the wasm bridge's job (A2 #896).
 *
 * **Read entries by the header's `entry_len`, never by a constant.** The three list types have
 * different entry sizes (route 84, ride 72, trip 76) and `routeList` has already grown once — the
 * auto-expiry tail was appended after the content CRC without a protocol bump. Stepping by the
 * announced length and decoding the prefix you know is the format's designed forward path; a
 * hard-coded stride turns the next additive change into a silent misparse.
 */

import { DecodeError, viewOf } from "./protocol";

// --- the shared list header (§7.4) --------------------------------------------

export const LIST_HEADER_LEN = 6;
export const LIST_VERSION = 2;

/** Entry sizes as of protocol v2 + auto-expiry. Encoders use them; decoders read `entryLen`. */
export const ROUTE_ENTRY_LEN = 84;
export const RIDE_ENTRY_LEN = 72;
export const TRIP_ENTRY_LEN = 76;

/** The smallest entry any list type uses — the header decoder's floor sanity check. */
const MIN_ENTRY_LEN = RIDE_ENTRY_LEN;

/**
 * ```text
 *   version    u8   = 2
 *   entry_len  u8   the entry size; readers step by it
 *   count      u16  entries actually in this object (after the device's cap)
 *   total      u16  full catalog size BEFORE the cap
 * ```
 *
 * `total > count` is the wire's way of saying the device dropped entries at its cap — surfaced as
 * a one-line warning rather than a silent "up to date".
 */
export interface ListHeader {
    count: number;
    total: number;
    entryLen: number;
}

export function encodeListHeader(h: ListHeader): Uint8Array {
    const out = new Uint8Array(LIST_HEADER_LEN);
    const view = new DataView(out.buffer);
    out[0] = LIST_VERSION;
    out[1] = h.entryLen;
    view.setUint16(2, h.count, true);
    view.setUint16(4, h.total, true);
    return out;
}

export function decodeListHeader(data: Uint8Array): ListHeader {
    if (data.length < LIST_HEADER_LEN) {
        throw new DecodeError("truncated", `list object is ${data.length} bytes, shorter than its 6-byte header.`);
    }
    if (data[0] !== LIST_VERSION) {
        throw new DecodeError("unknown-status", `list object version ${data[0]}, this client speaks ${LIST_VERSION}.`);
    }
    const entryLen = data[1];
    if (entryLen < MIN_ENTRY_LEN) {
        throw new DecodeError("unknown-status", `list entry_len ${entryLen} is below the ${MIN_ENTRY_LEN}-byte floor.`);
    }
    const view = viewOf(data);
    return { entryLen, count: view.getUint16(2, true), total: view.getUint16(4, true) };
}

/** Whether the device truncated this list at its cap — `total - count` entries were dropped. */
export function isTruncated(h: ListHeader): boolean {
    return h.total > h.count;
}

/**
 * Walk a list object, decoding each entry with `decodeEntry`.
 *
 * The stride is the header's `entryLen`, so a future firmware whose entries carry more fields
 * still walks correctly and each entry decodes the prefix this client understands.
 */
function decodeList<T>(data: Uint8Array, minEntryLen: number, what: string, decodeEntry: (slot: Uint8Array) => T) {
    const header = decodeListHeader(data);
    if (header.entryLen < minEntryLen) {
        throw new DecodeError("truncated", `${what} entries are ${header.entryLen} bytes, this client needs ${minEntryLen}.`);
    }
    const entries: T[] = [];
    for (let k = 0; k < header.count; k++) {
        const start = LIST_HEADER_LEN + k * header.entryLen;
        const slot = data.subarray(start, start + header.entryLen);
        if (slot.length < header.entryLen) {
            throw new DecodeError("truncated", `${what} claims ${header.count} entries but ends inside entry ${k}.`);
        }
        entries.push(decodeEntry(slot));
    }
    return { header, entries };
}

// --- routeList (§7.4) ---------------------------------------------------------

/**
 * One stored route, as the catalog reports it.
 *
 * `crc32` is the fingerprint of the stored OBCR bytes, computed by the device at upload commit —
 * `0` means "unknown" (a side-loaded file not yet fingerprinted). `expiresAt` and `retention` sit
 * *after* it and are deliberately outside its coverage: they are device-computed volatile state, so
 * a route whose countdown merely ticked must never read as "content changed".
 */
export interface RouteListEntry {
    objectId: number;
    byteLen: number;
    distanceM: number;
    ascentM: number;
    pointCount: number;
    waypointCount: number;
    name: string;
    crc32: number;
    /** Unix seconds the route auto-deletes at; `0` = never, or the clock has not started. */
    expiresAt: number;
    /** `0` never · `1` 1 day · `2` 1 week · `3` 2 weeks · `4` 1 month · `5` 2 months. */
    retention: number;
}

export function decodeRouteListEntry(slot: Uint8Array): RouteListEntry {
    const view = viewOf(slot);
    return {
        objectId: view.getUint16(0, true),
        byteLen: view.getUint32(4, true),
        distanceM: view.getUint32(8, true),
        ascentM: view.getUint32(12, true),
        pointCount: view.getUint32(16, true),
        waypointCount: view.getUint16(20, true),
        name: paddedName(slot, 22, 23, 48),
        crc32: view.getUint32(72, true),
        expiresAt: view.getUint32(76, true),
        retention: slot[80],
    };
}

export function encodeRouteListEntry(e: RouteListEntry): Uint8Array {
    const out = new Uint8Array(ROUTE_ENTRY_LEN);
    const view = new DataView(out.buffer);
    view.setUint16(0, e.objectId, true);
    view.setUint32(4, e.byteLen, true);
    view.setUint32(8, e.distanceM, true);
    view.setUint32(12, e.ascentM, true);
    view.setUint32(16, e.pointCount, true);
    view.setUint16(20, e.waypointCount, true);
    writePaddedName(out, 22, 23, 48, e.name);
    view.setUint32(72, e.crc32, true);
    view.setUint32(76, e.expiresAt, true);
    out[80] = e.retention;
    return out;
}

export function decodeRouteList(data: Uint8Array) {
    return decodeList(data, ROUTE_ENTRY_LEN, "routeList", decodeRouteListEntry);
}

// --- rideList (§7.4) ----------------------------------------------------------

/** One recorded ride, as the catalog reports it. Unchanged since protocol v1 — which is exactly
 *  why entry length is carried per-list rather than shared. */
export interface RideListEntry {
    objectId: number;
    byteLen: number;
    startTime: number;
    distanceM: number;
    movingTimeS: number;
    avgSpeedCms: number;
    climbM: number;
    name: string;
}

export function decodeRideListEntry(slot: Uint8Array): RideListEntry {
    const view = viewOf(slot);
    return {
        objectId: view.getUint16(0, true),
        byteLen: view.getUint32(4, true),
        startTime: view.getUint32(8, true),
        distanceM: view.getUint32(12, true),
        movingTimeS: view.getUint32(16, true),
        avgSpeedCms: view.getUint16(20, true),
        climbM: view.getUint16(22, true),
        name: paddedName(slot, 24, 25, 47),
    };
}

export function encodeRideListEntry(e: RideListEntry): Uint8Array {
    const out = new Uint8Array(RIDE_ENTRY_LEN);
    const view = new DataView(out.buffer);
    view.setUint16(0, e.objectId, true);
    view.setUint32(4, e.byteLen, true);
    view.setUint32(8, e.startTime, true);
    view.setUint32(12, e.distanceM, true);
    view.setUint32(16, e.movingTimeS, true);
    view.setUint16(20, e.avgSpeedCms, true);
    view.setUint16(22, e.climbM, true);
    writePaddedName(out, 24, 25, 47, e.name);
    return out;
}

export function decodeRideList(data: Uint8Array) {
    return decodeList(data, RIDE_ENTRY_LEN, "rideList", decodeRideListEntry);
}

// --- tripList (§7.4) ----------------------------------------------------------

/** One trip, as the catalog reports it. `stageCount` counts every stored stage including dangling
 *  refs, while the totals sum only the stages the device could resolve — so they legitimately
 *  disagree. */
export interface TripListEntry {
    objectId: number;
    byteLen: number;
    totalDistanceM: number;
    totalAscentM: number;
    stageCount: number;
    name: string;
    crc32: number;
}

export function decodeTripListEntry(slot: Uint8Array): TripListEntry {
    const view = viewOf(slot);
    return {
        objectId: view.getUint16(0, true),
        byteLen: view.getUint32(4, true),
        totalDistanceM: view.getUint32(8, true),
        totalAscentM: view.getUint32(12, true),
        stageCount: view.getUint16(16, true),
        name: paddedName(slot, 20, 21, 48),
        crc32: view.getUint32(72, true),
    };
}

export function encodeTripListEntry(e: TripListEntry): Uint8Array {
    const out = new Uint8Array(TRIP_ENTRY_LEN);
    const view = new DataView(out.buffer);
    view.setUint16(0, e.objectId, true);
    view.setUint32(4, e.byteLen, true);
    view.setUint32(8, e.totalDistanceM, true);
    view.setUint32(12, e.totalAscentM, true);
    view.setUint16(16, e.stageCount, true);
    writePaddedName(out, 20, 21, 48, e.name);
    view.setUint32(72, e.crc32, true);
    return out;
}

export function decodeTripList(data: Uint8Array) {
    return decodeList(data, TRIP_ENTRY_LEN, "tripList", decodeTripListEntry);
}

// --- the ride object (§7.2) ---------------------------------------------------

/** Absent-value sentinels for the v2 sensor fields. */
const NO_U8 = 0xff;
const NO_U16 = 0xffff;
/** `INT16_MIN` — the ride object's "no elevation" sentinel. */
const NO_ELEVATION = -32768;

/** One recorded point. Coordinates are degrees × 1e7 (a ~1 cm grid); `null` means the sensor was
 *  absent, dropped, or stale. */
export interface RidePoint {
    tOffsetS: number;
    lat1e7: number;
    lon1e7: number;
    eleM: number | null;
    hrBpm: number | null;
    cadenceRpm: number | null;
    powerW: number | null;
}

/** A downloaded ride. The per-ride sensor summary is v2-only and reads as all-`null` from a v1
 *  ride — a device that has never seen a strap keeps writing v1, and old v1 rides on the card must
 *  still list, download and delete, so both versions decode here. */
export interface RideObject {
    version: 1 | 2;
    name: string;
    startTime: number;
    distanceM: number;
    movingTimeS: number;
    avgSpeedCms: number;
    climbM: number;
    avgHr: number | null;
    maxHr: number | null;
    avgCadence: number | null;
    avgPower: number | null;
    maxPower: number | null;
    points: RidePoint[];
}

/**
 * Decode a ride object, v1 or v2.
 *
 * The byte length is fully determined by the version and the header, so a payload whose length
 * disagrees is rejected rather than parsed into plausible nonsense — the check the spec asks for
 * explicitly.
 */
export function decodeRideObject(data: Uint8Array): RideObject {
    if (data.length < 3) throw new DecodeError("truncated", `ride object is ${data.length} bytes.`);
    const version = data[0];
    if (version !== 1 && version !== 2) {
        throw new DecodeError("unknown-status", `ride object version ${version}; this client decodes 1 and 2.`);
    }
    const view = viewOf(data);
    const nameLen = view.getUint16(1, true);
    const fixed = version === 1 ? 23 : 31;
    const pointLen = version === 1 ? 14 : 18;
    if (data.length < fixed + nameLen) {
        throw new DecodeError("truncated", `ride object header needs ${fixed + nameLen} bytes, got ${data.length}.`);
    }
    // The name sits between `name_len` and the rest of the header, so every field after it is
    // relative to where the name ended.
    const at = 3 + nameLen;
    const pointCount = view.getUint32(at + 16, true);
    const expected = fixed + nameLen + pointLen * pointCount;
    if (data.length !== expected) {
        throw new DecodeError(
            "truncated",
            `ride object v${version} with ${pointCount} points should be ${expected} bytes, got ${data.length}.`,
        );
    }

    const sensorsAt = at + 20;
    const points: RidePoint[] = [];
    for (let i = 0; i < pointCount; i++) {
        const p = fixed + nameLen + i * pointLen;
        const ele = view.getInt16(p + 12, true);
        points.push({
            tOffsetS: view.getUint32(p, true),
            lat1e7: view.getInt32(p + 4, true),
            lon1e7: view.getInt32(p + 8, true),
            eleM: ele === NO_ELEVATION ? null : ele,
            hrBpm: version === 2 ? absent8(data[p + 14]) : null,
            cadenceRpm: version === 2 ? absent8(data[p + 15]) : null,
            powerW: version === 2 ? absent16(view.getUint16(p + 16, true)) : null,
        });
    }

    return {
        version,
        points,
        name: new TextDecoder().decode(data.subarray(3, 3 + nameLen)),
        startTime: view.getUint32(at, true),
        distanceM: view.getUint32(at + 4, true),
        movingTimeS: view.getUint32(at + 8, true),
        avgSpeedCms: view.getUint16(at + 12, true),
        climbM: view.getUint16(at + 14, true),
        avgHr: version === 2 ? absent8(data[sensorsAt]) : null,
        maxHr: version === 2 ? absent8(data[sensorsAt + 1]) : null,
        avgCadence: version === 2 ? absent8(data[sensorsAt + 2]) : null,
        // sensorsAt + 3 is the reserved `pad` byte keeping the u16 fields aligned.
        avgPower: version === 2 ? absent16(view.getUint16(sensorsAt + 4, true)) : null,
        maxPower: version === 2 ? absent16(view.getUint16(sensorsAt + 6, true)) : null,
    };
}

/** Encode a ride object. The device is the only producer in production; this exists so the
 *  loopback device can serve a real ride and so the fixtures round-trip. */
export function encodeRideObject(r: RideObject): Uint8Array {
    const name = new TextEncoder().encode(r.name);
    const fixed = r.version === 1 ? 23 : 31;
    const pointLen = r.version === 1 ? 14 : 18;
    const out = new Uint8Array(fixed + name.length + pointLen * r.points.length);
    const view = new DataView(out.buffer);
    out[0] = r.version;
    view.setUint16(1, name.length, true);
    out.set(name, 3);
    const at = 3 + name.length;
    view.setUint32(at, r.startTime, true);
    view.setUint32(at + 4, r.distanceM, true);
    view.setUint32(at + 8, r.movingTimeS, true);
    view.setUint16(at + 12, r.avgSpeedCms, true);
    view.setUint16(at + 14, r.climbM, true);
    view.setUint32(at + 16, r.points.length, true);
    if (r.version === 2) {
        const s = at + 20;
        out[s] = r.avgHr ?? NO_U8;
        out[s + 1] = r.maxHr ?? NO_U8;
        out[s + 2] = r.avgCadence ?? NO_U8;
        view.setUint16(s + 4, r.avgPower ?? NO_U16, true);
        view.setUint16(s + 6, r.maxPower ?? NO_U16, true);
    }
    r.points.forEach((pt, i) => {
        const p = fixed + name.length + i * pointLen;
        view.setUint32(p, pt.tOffsetS, true);
        view.setInt32(p + 4, pt.lat1e7, true);
        view.setInt32(p + 8, pt.lon1e7, true);
        view.setInt16(p + 12, pt.eleM ?? NO_ELEVATION, true);
        if (r.version === 2) {
            out[p + 14] = pt.hrBpm ?? NO_U8;
            out[p + 15] = pt.cadenceRpm ?? NO_U8;
            view.setUint16(p + 16, pt.powerW ?? NO_U16, true);
        }
    });
    return out;
}

// --- the trip object (§7.7) ---------------------------------------------------

export const TRIP_HEADER_LEN = 56;

/**
 * A trip: a name and an ordered list of **route object ids**, never route bytes.
 *
 * Dangling stages — a member route deleted on its own — are tolerated on read and served verbatim;
 * the device never rewrites a stored trip. Compaction happens when the *peer* re-uploads the trip
 * built from resolvable stages, which is why this decoder keeps every id it finds.
 */
export interface TripObject {
    name: string;
    stages: number[];
}

export function decodeTripObject(data: Uint8Array): TripObject {
    if (data.length < TRIP_HEADER_LEN) {
        throw new DecodeError("truncated", `trip object is ${data.length} bytes, shorter than its 56-byte header.`);
    }
    if (data[0] !== 1) {
        throw new DecodeError("unknown-status", `trip object version ${data[0]}; this client decodes 1.`);
    }
    const view = viewOf(data);
    const stageCount = view.getUint16(2, true);
    const expected = TRIP_HEADER_LEN + 2 * stageCount;
    if (data.length !== expected) {
        throw new DecodeError("truncated", `trip with ${stageCount} stages should be ${expected} bytes, got ${data.length}.`);
    }
    const stages: number[] = [];
    for (let i = 0; i < stageCount; i++) stages.push(view.getUint16(TRIP_HEADER_LEN + i * 2, true));
    return { name: paddedName(data, 4, 5, 48), stages };
}

export function encodeTripObject(t: TripObject): Uint8Array {
    const out = new Uint8Array(TRIP_HEADER_LEN + 2 * t.stages.length);
    const view = new DataView(out.buffer);
    out[0] = 1;
    view.setUint16(2, t.stages.length, true);
    writePaddedName(out, 4, 5, 48, t.name);
    t.stages.forEach((id, i) => view.setUint16(TRIP_HEADER_LEN + i * 2, id, true));
    return out;
}

// --- name helpers -------------------------------------------------------------

/** Read a `name_len u8` + zero-padded UTF-8 field, clamping a bogus length to the field's cap. */
function paddedName(data: Uint8Array, lenAt: number, nameAt: number, cap: number): string {
    const len = Math.min(data[lenAt], cap);
    return new TextDecoder().decode(data.subarray(nameAt, nameAt + len));
}

/**
 * Write a `name_len u8` + zero-padded UTF-8 field, truncating an over-long name.
 *
 * Truncation is on a **byte** boundary, matching the firmware encoder — which can split a
 * multi-byte character. Names are capped at the source (48 bytes, the OBCR field), so this is a
 * backstop rather than a path anything travels.
 */
function writePaddedName(out: Uint8Array, lenAt: number, nameAt: number, cap: number, name: string): void {
    const bytes = new TextEncoder().encode(name);
    const n = Math.min(bytes.length, cap);
    out[lenAt] = n;
    out.set(bytes.subarray(0, n), nameAt);
}

function absent8(v: number): number | null {
    return v === NO_U8 ? null : v;
}

function absent16(v: number): number | null {
    return v === NO_U16 ? null : v;
}
