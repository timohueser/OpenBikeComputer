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

/** Absent-value sentinels in both samples and the final summary footer. */
const NO_U8 = 0xff;
const NO_U16 = 0xffff;
const RIDE_SAMPLE_LEN = 20;
const RIDE_FOOTER_LEN = 84;
const RIDE_NAME_CAP = 48;

/** One recorded point. Coordinates are degrees × 1e7 (a ~1 cm grid); `null` means the sensor was
 *  absent, dropped, or stale. */
export interface RidePoint {
    /** The recorded wrapping monotonic clock, in milliseconds. */
    tMs: number;
    latMicrodegrees: number;
    lonMicrodegrees: number;
    elevationM: number;
    segmentStart: boolean;
    hrBpm: number | null;
    cadenceRpm: number | null;
    powerW: number | null;
}

/** A downloaded v3 ride: the recorded sample bytes followed by one fixed summary footer. */
export interface RideObject {
    version: 3;
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
 * Decode the only ride-object format: verbatim 20-byte samples followed by the fixed 84-byte v3
 * footer. The footer's point count determines the complete object length.
 */
export function decodeRideObject(data: Uint8Array): RideObject {
    if (data.length < RIDE_FOOTER_LEN) throw new ObjectDecodeError(`ride object is ${data.length} bytes.`);
    const view = viewOf(data);
    const footer = data.length - RIDE_FOOTER_LEN;
    if (String.fromCharCode(...data.subarray(footer, footer + 4)) !== "OBRF") {
        throw new ObjectDecodeError("ride object has no OBRF footer.");
    }
    const version = data[footer + 4];
    if (version !== 3) throw new ObjectDecodeError(`ride object version ${version}; this client decodes 3.`);
    const nameLen = data[footer + 5];
    if (
        nameLen > RIDE_NAME_CAP ||
        view.getUint16(footer + 6, true) !== RIDE_FOOTER_LEN ||
        data[footer + 31] !== 0 ||
        data.subarray(footer + 36 + nameLen).some((byte) => byte !== 0)
    ) {
        throw new ObjectDecodeError("ride object has a non-canonical summary footer.");
    }
    let name: string;
    try {
        name = new TextDecoder("utf-8", { fatal: true }).decode(data.subarray(footer + 36, footer + 36 + nameLen));
    } catch {
        throw new ObjectDecodeError("ride object name is not UTF-8.");
    }
    const pointCount = view.getUint32(footer + 24, true);
    const expected = pointCount * RIDE_SAMPLE_LEN + RIDE_FOOTER_LEN;
    if (data.length !== expected) {
        throw new ObjectDecodeError(`ride object with ${pointCount} points should be ${expected} bytes, got ${data.length}.`);
    }

    const points: RidePoint[] = [];
    for (let i = 0; i < pointCount; i++) {
        const p = i * RIDE_SAMPLE_LEN;
        const flags = view.getUint16(p + 10, true);
        if ((flags & ~1) !== 0) throw new ObjectDecodeError(`ride sample ${i} has reserved flags set.`);
        points.push({
            lonMicrodegrees: view.getInt32(p, true),
            latMicrodegrees: view.getInt32(p + 4, true),
            elevationM: view.getInt16(p + 8, true),
            segmentStart: (flags & 1) !== 0,
            tMs: view.getUint32(p + 12, true),
            hrBpm: absent8(data[p + 16]),
            cadenceRpm: absent8(data[p + 17]),
            powerW: absent16(view.getUint16(p + 18, true)),
        });
    }

    return {
        version: 3,
        points,
        name,
        startTime: view.getUint32(footer + 8, true),
        distanceM: view.getUint32(footer + 12, true),
        movingTimeS: view.getUint32(footer + 16, true),
        avgSpeedCms: view.getUint16(footer + 20, true),
        climbM: view.getUint16(footer + 22, true),
        avgHr: absent8(data[footer + 28]),
        maxHr: absent8(data[footer + 29]),
        avgCadence: absent8(data[footer + 30]),
        avgPower: absent16(view.getUint16(footer + 32, true)),
        maxPower: absent16(view.getUint16(footer + 34, true)),
    };
}

/** Encode a v3 object for the loopback device and byte-contract tests. */
export function encodeRideObject(r: RideObject): Uint8Array {
    const name = clippedUtf8(r.name, RIDE_NAME_CAP);
    const footer = r.points.length * RIDE_SAMPLE_LEN;
    const out = new Uint8Array(footer + RIDE_FOOTER_LEN);
    const view = new DataView(out.buffer);
    r.points.forEach((pt, i) => {
        const p = i * RIDE_SAMPLE_LEN;
        view.setInt32(p, pt.lonMicrodegrees, true);
        view.setInt32(p + 4, pt.latMicrodegrees, true);
        view.setInt16(p + 8, pt.elevationM, true);
        view.setUint16(p + 10, pt.segmentStart ? 1 : 0, true);
        view.setUint32(p + 12, pt.tMs, true);
        out[p + 16] = pt.hrBpm ?? NO_U8;
        out[p + 17] = pt.cadenceRpm ?? NO_U8;
        view.setUint16(p + 18, pt.powerW ?? NO_U16, true);
    });
    out.set([0x4f, 0x42, 0x52, 0x46, 3, name.length], footer);
    view.setUint16(footer + 6, RIDE_FOOTER_LEN, true);
    view.setUint32(footer + 8, r.startTime, true);
    view.setUint32(footer + 12, r.distanceM, true);
    view.setUint32(footer + 16, r.movingTimeS, true);
    view.setUint16(footer + 20, r.avgSpeedCms, true);
    view.setUint16(footer + 22, r.climbM, true);
    view.setUint32(footer + 24, r.points.length, true);
    out[footer + 28] = r.avgHr ?? NO_U8;
    out[footer + 29] = r.maxHr ?? NO_U8;
    out[footer + 30] = r.avgCadence ?? NO_U8;
    view.setUint16(footer + 32, r.avgPower ?? NO_U16, true);
    view.setUint16(footer + 34, r.maxPower ?? NO_U16, true);
    out.set(name, footer + 36);
    return out;
}

function clippedUtf8(value: string, cap: number): Uint8Array {
    const encoded = new TextEncoder().encode(value);
    let end = Math.min(encoded.length, cap);
    while (end > 0 && (encoded[end] & 0xc0) === 0x80) end--;
    return encoded.subarray(0, end);
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
    stages: bigint[];
}

export function decodeTripObject(data: Uint8Array): TripObject {
    if (data.length < TRIP_HEADER_LEN) {
        throw new ObjectDecodeError(`trip object is ${data.length} bytes, shorter than its 56-byte header.`);
    }
    if (data[0] !== 2) {
        throw new ObjectDecodeError(`trip object version ${data[0]}; this client decodes 2.`);
    }
    const view = viewOf(data);
    const stageCount = view.getUint16(2, true);
    const expected = TRIP_HEADER_LEN + 8 * stageCount;
    if (data.length !== expected) {
        throw new ObjectDecodeError(`trip with ${stageCount} stages should be ${expected} bytes, got ${data.length}.`);
    }
    const stages: bigint[] = [];
    for (let i = 0; i < stageCount; i++) stages.push(view.getBigUint64(TRIP_HEADER_LEN + i * 8, true));
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
    const out = new Uint8Array(TRIP_HEADER_LEN + 8 * t.stages.length);
    const view = new DataView(out.buffer);
    out[0] = 2;
    view.setUint16(2, t.stages.length, true);
    writePaddedName(out, 4, 5, 48, t.name);
    t.stages.forEach((id, i) => view.setBigUint64(TRIP_HEADER_LEN + i * 8, id, true));
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
