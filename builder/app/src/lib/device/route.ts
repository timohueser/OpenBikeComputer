/**
 * A dropped GPX, on its way to the device (C4, #903).
 *
 * Two rules shape this, both of them older than this issue:
 *
 * - **The device never parses XML** ([interface spec](../../../../../specs/obc-ble-interface-spec.md)
 *   principle 3). A route crosses the wire as an [OBCR](../../../../../specs/OBCR_Spec.md) file and is
 *   written to storage verbatim, so the peer converts. Here that is A2's wasm bridge, which is the
 *   same `obc-route` code the device and the CLI run — byte-identical, pinned by `bridge.test.ts`.
 * - **Show what was dropped before sending it.** A GPX file's name says nothing about what is
 *   inside it, and the rider is about to put it on the thing they will navigate by. The distance,
 *   the ascent and the point count come from the OBCR header the conversion just produced — not
 *   from re-reading the GPX — so what is shown is exactly what the device will read back.
 */

import { gpxToObcr } from "../convert/bridge";
import { viewOf } from "../usb/protocol";

/** The route name field's cap, and the OBCR header's own (`Name Len`, §1). */
export const ROUTE_NAME_MAX = 48;

/** The base header both OBCR versions share. v2 adds 16 bytes of extension after it. */
const HEADER_BASE_LEN = 112;
const MAGIC = 0x4f424352; // "OBCR", big-endian read of the four ASCII bytes

/** The header fields worth showing a rider, read back out of the produced file (`OBCR_Spec.md` §1). */
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
 * Deliberately a *reader*, not a validator: the file it is handed came out of `gpx_to_obcr` a
 * moment ago, so the checks here exist to catch a wrong file being fed in (someone's `.obcm`, a
 * truncated download), not to re-verify the converter.
 */
export function decodeRouteHeader(bytes: Uint8Array): RouteHeader {
    if (bytes.length < HEADER_BASE_LEN) {
        throw new RouteError(`That file is ${bytes.length} bytes — too short to be a route.`);
    }
    const view = viewOf(bytes);
    if (view.getUint32(0, false) !== MAGIC) throw new RouteError("That file is not an OBCR route.");
    const version = bytes[4];
    if (version !== 1 && version !== 2) {
        throw new RouteError(`That route is OBCR v${version}; this page writes v1 and v2.`);
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
 * Convert a dropped file to OBCR and read back what it contains.
 *
 * The route's name is the file's stem, trimmed to the format's 48 **bytes** — the OBCR header
 * measures the field in bytes, so trimming by JavaScript string length would produce a name the
 * converter then truncates differently. Anything the conversion rejects arrives as a
 * `ConvertError` with a message written for a rider (`convert/bridge.ts`); it is not re-wrapped,
 * because that message is already the right one.
 */
export async function prepareRoute(file: File): Promise<PreparedRoute> {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const obcr = await gpxToObcr(bytes, routeNameFrom(file.name));
    return { obcr, header: decodeRouteHeader(obcr), sourceName: file.name };
}

/** A file name turned into a route name: no extension, no path, and inside the format's byte cap. */
export function routeNameFrom(filename: string): string {
    const stem = filename.replace(/\.[^./\\]+$/, "").replace(/^.*[\\/]/, "").trim();
    return truncateUtf8(stem || "Route", ROUTE_NAME_MAX);
}

/** Trim to `maxBytes` of UTF-8 without splitting a codepoint — the rule `obc-route` applies too. */
export function truncateUtf8(text: string, maxBytes: number): string {
    const encoder = new TextEncoder();
    if (encoder.encode(text).length <= maxBytes) return text;
    let out = "";
    let used = 0;
    for (const ch of text) {
        const size = encoder.encode(ch).length;
        if (used + size > maxBytes) break;
        out += ch;
        used += size;
    }
    return out;
}
