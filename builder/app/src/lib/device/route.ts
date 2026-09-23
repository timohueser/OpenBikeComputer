/**
 * A dropped GPX, on its way to the device.
 *
 * Two rules shape this:
 *
 * - **The device never parses XML.** A route crosses the wire as an OBCR file and is written to
 *   storage verbatim, so the peer converts. Here that is the wasm bridge, which is the same
 *   `obc-route` code the device and the CLI run, pinned byte-for-byte by `bridge.test.ts`.
 * - **Show what was dropped before sending it.** A GPX file's name says nothing about what is inside
 *   it, and the rider is about to put it on the thing they will navigate by. The distance, the
 *   ascent and the point count come from the OBCR header the conversion just produced, not from
 *   re-reading the GPX.
 */

import { gpxToObcr } from "../convert/bridge";
import { truncateUtf8 } from "../format";
import { viewOf } from "../usb/protocol";

/** The route name field's cap, and the OBCR header's own. */
export const ROUTE_NAME_MAX = 48;

/** The device's bike types in the device's order. The index is the OBCR header's type byte. */
export const BIKE_TYPES = ["Road", "Gravel", "MTB", "Touring"] as const;

const BIKE_TYPE_KEY = "obcm.routeBikeType";

/** The bike type last picked for a dropped route. Road when there is no pick or no storage. */
export function rememberedBikeType(): number {
    try {
        const bike = Number(globalThis.localStorage?.getItem(BIKE_TYPE_KEY));
        return Number.isInteger(bike) && bike >= 0 && bike < BIKE_TYPES.length ? bike : 0;
    } catch {
        return 0;
    }
}

export function rememberBikeType(bike: number): void {
    try {
        globalThis.localStorage?.setItem(BIKE_TYPE_KEY, String(bike));
    } catch {
        // The pick still applies to this page; denied storage means the next visit starts at Road.
    }
}

/** Complete current OBCR header, including waypoint and Visit descriptors. */
const HEADER_LEN = 160;
const MAGIC = 0x4f424352; // "OBCR", big-endian read of the four ASCII bytes

/** The header fields worth showing a rider, read back out of the produced file. */
export interface RouteHeader {
    version: number;
    name: string;
    /** Distinct stored points. Decimated for drawing; the distances below are not. */
    pointCount: number;
    /** Meters, exact — computed at conversion from **all** raw GPX points, not the stored ones. */
    distanceM: number;
    /** Meters of ascent, smoothed, likewise from the raw points. */
    ascentM: number;
    descentM: number;
}

export class RouteError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "RouteError";
    }
}

/**
 * Read an OBCR header.
 *
 * Deliberately a *reader*, not a validator: the file it is handed came out of `gpx_to_obcr` a moment
 * ago, so the checks here exist to catch a wrong file being fed in, not to re-verify the converter.
 */
export function decodeRouteHeader(bytes: Uint8Array): RouteHeader {
    if (bytes.length < HEADER_LEN) {
        throw new RouteError(`That file is ${bytes.length} bytes — too short to be a route.`);
    }
    const view = viewOf(bytes);
    if (view.getUint32(0, false) !== MAGIC) throw new RouteError("That file is not an OBCR route.");
    const version = bytes[4];
    if (version !== 5) {
        throw new RouteError(`That route is OBCR v${version}; this page writes v5.`);
    }
    const nameLen = Math.min(bytes[6], ROUTE_NAME_MAX);
    return {
        version,
        name: new TextDecoder().decode(bytes.subarray(64, 64 + nameLen)),
        pointCount: view.getUint32(32, true),
        distanceM: view.getUint32(36, true),
        ascentM: view.getUint32(40, true),
        descentM: view.getUint32(44, true),
    };
}

/** A converted route, ready to announce and to send. */
export interface PreparedRoute {
    /** The OBCR bytes, exactly as they will be stored. */
    readonly obcr: Uint8Array;
    readonly header: RouteHeader;
    /** The dropped file's name, for the "what did I just drop" line. */
    readonly sourceName: string;
}

/**
 * Convert a dropped file to an OBCR typed `bike` and read back what it contains.
 *
 * The route's name is the file's stem, trimmed to the format's 48 **bytes** — the OBCR header
 * measures the field in bytes, so trimming by JavaScript string length would produce a name the
 * converter then truncates differently. Anything the conversion rejects arrives as a `ConvertError`
 * with a message written for a rider; it is not re-wrapped, because that message is already right.
 */
export async function prepareRoute(file: File, bike: number): Promise<PreparedRoute> {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const obcr = await gpxToObcr(bytes, routeNameFrom(file.name), bike);
    return { obcr, header: decodeRouteHeader(obcr), sourceName: file.name };
}

/** A file name turned into a route name: no extension, no path, and inside the format's byte cap. */
export function routeNameFrom(filename: string): string {
    const stem = filename.replace(/\.[^./\\]+$/, "").replace(/^.*[\\/]/, "").trim();
    return truncateUtf8(stem || "Route", ROUTE_NAME_MAX);
}
