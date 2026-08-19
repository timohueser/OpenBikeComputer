/**
 * Object **payload** layouts: the ride object and the trip object.
 *
 * Not the wire. Protocol v4 carries an object as opaque bytes with a kind, a display name and a
 * whole-payload CRC (`FLAT_Store_Protocol.md` §3), so nothing in this file crosses a frame boundary
 * or has a §3 offset — it is what the bytes *inside* one object mean, and only for the two kinds
 * this app has to look inside.
 *
 * Routes, maps and firmware images are absent on purpose: an `.obcr`, an `.obcm` and an `UPDATE.BIN`
 * are written verbatim and read back verbatim, so there is nothing here to decode. The browser's
 * OBCR encoding is the wasm bridge's job, and `device/route.ts` reads its header.
 *
 * The **list objects are gone with the v1 wire**. There is no `routeList`, `rideList` or `tripList`
 * any more: `LIST` is a control response carrying 88-byte catalog entries (§3.3), and what a client
 * can know about an object without downloading it is exactly what that entry holds — id, revision,
 * length, CRC, kind, flags and display name. The richer per-kind metadata those objects used to
 * carry (a route's distance and ascent, a ride's start time and moving time) lives in the payload,
 * and a client that wants it reads the payload.
 */

import { viewOf } from "./protocol";

/**
 * A payload this build cannot read.
 *
 * Object payloads are not the wire. §3 carries an object as opaque bytes with a kind, a name and a
 * CRC, so nothing below is a protocol failure the device could have answered differently — a ride
 * this page cannot decode is a page behind its device, and it needs its own error rather than one of
 * §3.9's codes.
 */
export class ObjectDecodeError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "ObjectDecodeError";
    }
}

// --- the ride object ---------------------------------------------------

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
    if (data.length < 3) throw new ObjectDecodeError(`ride object is ${data.length} bytes.`);
    const version = data[0];
    if (version !== 1 && version !== 2) {
        throw new ObjectDecodeError(`ride object version ${version}; this client decodes 1 and 2.`);
    }
    const view = viewOf(data);
    const nameLen = view.getUint16(1, true);
    const fixed = version === 1 ? 23 : 31;
    const pointLen = version === 1 ? 14 : 18;
    if (data.length < fixed + nameLen) {
        throw new ObjectDecodeError(`ride object header needs ${fixed + nameLen} bytes, got ${data.length}.`);
    }
    // The name sits between `name_len` and the rest of the header, so every field after it is
    // relative to where the name ended.
    const at = 3 + nameLen;
    const pointCount = view.getUint32(at + 16, true);
    const expected = fixed + nameLen + pointLen * pointCount;
    if (data.length !== expected) {
        throw new ObjectDecodeError(
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

// --- the trip object ---------------------------------------------------

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
        throw new ObjectDecodeError(`trip object is ${data.length} bytes, shorter than its 56-byte header.`);
    }
    if (data[0] !== 1) {
        throw new ObjectDecodeError(`trip object version ${data[0]}; this client decodes 1.`);
    }
    const view = viewOf(data);
    const stageCount = view.getUint16(2, true);
    const expected = TRIP_HEADER_LEN + 2 * stageCount;
    if (data.length !== expected) {
        throw new ObjectDecodeError(`trip with ${stageCount} stages should be ${expected} bytes, got ${data.length}.`);
    }
    const stages: number[] = [];
    for (let i = 0; i < stageCount; i++) stages.push(view.getUint16(TRIP_HEADER_LEN + i * 2, true));
    return { name: paddedName(data, 4, 5, 48), stages };
}

export function encodeTripObject(t: TripObject): Uint8Array {
    // **Refused here, not left to call-site discipline.** `setUint16` wraps silently, so a trip with
    // 65,536 stages would encode as one with zero and the device would commit a trip that is not the
    // trip it was given — a wrong object rather than a rejected one, which is the failure direction
    // worth spending a branch on. Every caller today is far under the cap; that is exactly why the
    // check belongs in the encoder, where it stays true when a caller stops being careful.
    if (t.stages.length > 0xffff) {
        throw new RangeError(`a trip carries at most 65535 stages; this one has ${t.stages.length}`);
    }
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
