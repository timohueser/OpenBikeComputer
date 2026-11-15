/**
 * Object **payload** layout: the ride object.
 *
 * Not the wire. The protocol carries an object as opaque bytes with a kind, a display name and a
 * whole-payload CRC, so nothing in this file crosses a frame boundary — it is what the bytes
 * *inside* one object mean, and only for the one kind this app has to look inside.
 *
 * Routes, maps and firmware images are absent on purpose: an `.obcr`, an `.obcm` and an `UPDATE.BIN`
 * are written verbatim and read back verbatim. The browser's OBCR encoding is the wasm bridge's job,
 * and `device/route.ts` reads its header.
 *
 * There is no list object: `LIST` is a control response carrying 88-byte catalog entries, and what a
 * client can know about an object without downloading it is exactly what that entry holds. The
 * richer per-kind metadata lives in the payload, and a client that wants it reads the payload.
 */

import { viewOf } from "./protocol";

/**
 * A payload this build cannot read.
 *
 * Object payloads are not the wire, so nothing below is a protocol failure the device could have
 * answered differently — a ride this page cannot decode is a page behind its device, and it needs
 * its own error rather than one of the wire's codes.
 */
export class ObjectDecodeError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "ObjectDecodeError";
    }
}

/** Absent-value sentinels in both samples and the final summary footer. */
const NO_U8 = 0xff;
const NO_U16 = 0xffff;
const NO_U32 = 0xffffffff;
const RIDE_SAMPLE_LEN = 20;
const RIDE_FOOTER_LEN = 154;
const RIDE_NAME_CAP = 48;
const RIDE_NAME_AT = 42;
const RIDE_TRIP_AT = 90;
const RIDE_LIMITS_AT = 150;

/** One recorded point. Coordinates are microdegrees (`specs/ble-interface.md` §7.2); `null` means the sensor was
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

/** The trip day a ride started on (spec §7.2). */
export interface RideTrip {
    /** Never 0: key 0 means "no trip" on the wire. */
    key: bigint;
    /** 0-based. */
    dayIndex: number;
    dayCount: number;
    /** The trip's name when the ride was saved; empty when the device no longer held the trip. */
    name: string;
}

/** The rider's effort limits when the ride started (spec §7.2); `null` is not set. */
export interface RideEffortLimits {
    maxHrBpm: number | null;
    ftpW: number | null;
}

/** A downloaded v6 ride: the recorded sample bytes followed by one fixed summary footer. */
export interface RideObject {
    version: 6;
    /** Synthetic onboarding ride; preserved with the recorded payload. */
    isDemo: boolean;
    name: string;
    startTime: number;
    distanceM: number;
    movingTimeS: number;
    avgSpeedCms: number;
    climbM: number;
    descentM: number;
    avgHr: number | null;
    maxHr: number | null;
    avgCadence: number | null;
    avgPower: number | null;
    maxPower: number | null;
    /** The ride's energy from power; `null` without power data. */
    energyKj: number | null;
    /** The bike type current at the start, `0..=3` (Road, Gravel, MTB, Touring). */
    bikeType: number;
    trip: RideTrip | null;
    effortLimits: RideEffortLimits;
    points: RidePoint[];
}

/**
 * Decode the only ride-object format: verbatim 20-byte samples followed by the fixed 154-byte v6
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
    if (version !== 6) throw new ObjectDecodeError(`ride object version ${version}; this client decodes 6.`);
    const limits = footer + RIDE_LIMITS_AT;
    const flags = data[footer + 33];
    if (view.getUint16(footer + 6, true) !== RIDE_FOOTER_LEN || (flags & ~1) !== 0 || data[limits + 1] !== 0) {
        throw new ObjectDecodeError("ride object has a non-canonical summary footer.");
    }
    const name = footerName(data, footer + RIDE_NAME_AT, data[footer + 5]);
    const trip = footer + RIDE_TRIP_AT;
    const tripKey = view.getBigUint64(trip, true);
    const dayIndex = data[trip + 8];
    const dayCount = data[trip + 9];
    const bikeType = data[trip + 10];
    const tripName = footerName(data, trip + 12, data[trip + 11]);
    const noTrip = tripKey === 0n && dayIndex === 0 && dayCount === 0 && tripName === "";
    if (bikeType > 3 || (tripKey === 0n ? !noTrip : dayIndex >= dayCount)) {
        throw new ObjectDecodeError("ride object has a non-canonical summary footer.");
    }
    const pointCount = view.getUint32(footer + 26, true);
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
        version: 6,
        isDemo: (flags & 1) !== 0,
        points,
        name,
        startTime: view.getUint32(footer + 8, true),
        distanceM: view.getUint32(footer + 12, true),
        movingTimeS: view.getUint32(footer + 16, true),
        avgSpeedCms: view.getUint16(footer + 20, true),
        climbM: view.getUint16(footer + 22, true),
        descentM: view.getUint16(footer + 24, true),
        avgHr: absent8(data[footer + 30]),
        maxHr: absent8(data[footer + 31]),
        avgCadence: absent8(data[footer + 32]),
        avgPower: absent16(view.getUint16(footer + 34, true)),
        maxPower: absent16(view.getUint16(footer + 36, true)),
        energyKj: absent32(view.getUint32(footer + 38, true)),
        bikeType,
        trip: tripKey === 0n ? null : { key: tripKey, dayIndex, dayCount, name: tripName },
        effortLimits: { maxHrBpm: data[limits] || null, ftpW: view.getUint16(limits + 2, true) || null },
    };
}

/** A zero-padded UTF-8 name field of `RIDE_NAME_CAP` bytes, `len` of them used. */
function footerName(data: Uint8Array, at: number, len: number): string {
    if (len > RIDE_NAME_CAP || data.subarray(at + len, at + RIDE_NAME_CAP).some((byte) => byte !== 0)) {
        throw new ObjectDecodeError("ride object has a non-canonical summary footer.");
    }
    try {
        return new TextDecoder("utf-8", { fatal: true }).decode(data.subarray(at, at + len));
    } catch {
        throw new ObjectDecodeError("ride object name is not UTF-8.");
    }
}

/** Encode a v6 object for the loopback device and byte-contract tests. */
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
    out.set([0x4f, 0x42, 0x52, 0x46, 6, name.length], footer);
    view.setUint16(footer + 6, RIDE_FOOTER_LEN, true);
    view.setUint32(footer + 8, r.startTime, true);
    view.setUint32(footer + 12, r.distanceM, true);
    view.setUint32(footer + 16, r.movingTimeS, true);
    view.setUint16(footer + 20, r.avgSpeedCms, true);
    view.setUint16(footer + 22, r.climbM, true);
    view.setUint16(footer + 24, r.descentM, true);
    view.setUint32(footer + 26, r.points.length, true);
    out[footer + 30] = r.avgHr ?? NO_U8;
    out[footer + 31] = r.maxHr ?? NO_U8;
    out[footer + 32] = r.avgCadence ?? NO_U8;
    out[footer + 33] = r.isDemo ? 1 : 0;
    view.setUint16(footer + 34, r.avgPower ?? NO_U16, true);
    view.setUint16(footer + 36, r.maxPower ?? NO_U16, true);
    view.setUint32(footer + 38, r.energyKj ?? NO_U32, true);
    out.set(name, footer + RIDE_NAME_AT);
    const trip = footer + RIDE_TRIP_AT;
    if (r.trip) {
        const tripName = clippedUtf8(r.trip.name, RIDE_NAME_CAP);
        view.setBigUint64(trip, r.trip.key, true);
        out[trip + 8] = r.trip.dayIndex;
        out[trip + 9] = r.trip.dayCount;
        out[trip + 11] = tripName.length;
        out.set(tripName, trip + 12);
    }
    out[trip + 10] = r.bikeType;
    out[footer + RIDE_LIMITS_AT] = r.effortLimits.maxHrBpm ?? 0;
    view.setUint16(footer + RIDE_LIMITS_AT + 2, r.effortLimits.ftpW ?? 0, true);
    return out;
}

function clippedUtf8(value: string, cap: number): Uint8Array {
    const encoded = new TextEncoder().encode(value);
    let end = Math.min(encoded.length, cap);
    while (end > 0 && (encoded[end] & 0xc0) === 0x80) end--;
    return encoded.subarray(0, end);
}

function absent8(v: number): number | null {
    return v === NO_U8 ? null : v;
}

function absent16(v: number): number | null {
    return v === NO_U16 ? null : v;
}

function absent32(v: number): number | null {
    return v === NO_U32 ? null : v;
}
