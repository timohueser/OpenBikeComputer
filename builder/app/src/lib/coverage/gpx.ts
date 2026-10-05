// Reading a GPX file into corridor points.
//
// The corridor panel takes "a route" from two places — a `.gpx` upload and the routes
// stored on a connected device — and both end as a named polyline in integer
// microdegrees. This module is the upload half. The web planner reads the same files
// with `readGpx`, which keeps every point and reads the `<wpt>`s.
//
// It is a **string scanner, not an XML parser**, and that is a decision rather than a
// shortcut. The only facts a corridor needs are the `lat`/`lon` attribute pairs of the
// `<trkpt>`/`<rtept>` elements and a display name. A DOM parse would chain this module
// to a browser, putting the one piece of the corridor flow that is pure data juggling
// out of reach of the unit suite.
//
// `parseGpx` **decimates** points to a ceiling. A recorded track can
// carry a point per second, and the corridor test is per segment per candidate cell. At
// the grid's cell sizes, dropping intermediate points moves the corridor's edge by
// metres. The ends are always kept.

import type { LatLon } from "../catalog/corridor";
import { M_PER_DEG } from "../catalog/corridor";

/** One GPX point; `ele` is the `<ele>` height in metres, when the file has one. */
export type GpxPoint = LatLon & { ele?: number };

/** One `<wpt>` with its `<name>` and its `<desc>` (else `<cmt>`), when it has them. */
export type GpxWaypoint = LatLon & { name?: string; note?: string };

/** One route as the corridor panel lists it. */
export interface GpxRoute {
    name: string;
    /** Integer microdegrees, decimated to {@link MAX_ROUTE_POINTS}. */
    points: LatLon[];
    /** Approximate length of the polyline, km — display only. */
    distanceKm: number;
}

/** The point ceiling after decimation. A chosen value: 2048 segments resolve a
 *  corridor's cell set exactly at every size the grid permits. */
export const MAX_ROUTE_POINTS = 2048;

/** A file that yielded no usable route. The message is shown verbatim. */
export class GpxError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "GpxError";
    }
}

/** `lat`/`lon` attributes out of one element's tag text, either order, either quote
 *  style. `null` for a malformed pair — the caller counts those. */
function pointOf(tag: string): LatLon | null {
    const lat = /\blat\s*=\s*["']([^"']+)["']/.exec(tag);
    const lon = /\blon\s*=\s*["']([^"']+)["']/.exec(tag);
    if (!lat || !lon) return null;
    const latDeg = Number(lat[1]);
    const lonDeg = Number(lon[1]);
    if (!Number.isFinite(latDeg) || !Number.isFinite(lonDeg)) return null;
    if (Math.abs(latDeg) > 90 || Math.abs(lonDeg) > 180) return null;
    return { lat: Math.round(latDeg * 1e6), lon: Math.round(lonDeg * 1e6) };
}

const WPT = /<wpt\b([^>]*?)(?:\/>|>([\s\S]*?)<\/wpt\s*>)/g;

/** The first non-empty `<tag>` text in `text`. GPX is XML, so the five predefined entities
 *  are all that can appear un-escaped in it. */
function firstText(text: string, tag = "name"): string | null {
    const value = new RegExp(`<${tag}>\\s*([\\s\\S]*?)\\s*<\\/${tag}>`).exec(text)?.[1].trim();
    if (!value) return null;
    return value
        .replace(/&lt;/g, "<")
        .replace(/&gt;/g, ">")
        .replace(/&quot;/g, '"')
        .replace(/&apos;/g, "'")
        .replace(/&amp;/g, "&");
}

/** The first `<name>` inside the first `<trk>`/`<rte>`, else the file-level one. A
 *  waypoint's name never names the route. */
function nameOf(text: string): string | null {
    const scoped = /<(?:trk|rte)\b[^>]*>([\s\S]*?)<\/(?:trk|rte)>/.exec(text)?.[1];
    const file = text.replace(WPT, "");
    for (const within of scoped === undefined ? [file] : [scoped, file]) {
        const name = firstText(within);
        if (name) return name;
    }
    return null;
}

/** Equirectangular polyline length — the same small-angle arithmetic the
 *  corridor test itself uses, so the two numbers cannot disagree in kind. */
function lengthKm(points: LatLon[]): number {
    let m = 0;
    for (let k = 1; k < points.length; k++) {
        const a = points[k - 1];
        const b = points[k];
        const cos = Math.cos((((a.lat + b.lat) / 2) * Math.PI) / 180e6);
        const dLat = ((b.lat - a.lat) * M_PER_DEG) / 1e6;
        const dLon = ((b.lon - a.lon) * M_PER_DEG * cos) / 1e6;
        m += Math.hypot(dLat, dLon);
    }
    return m / 1000;
}

/** Every nth point, ends always kept. */
function decimate(points: LatLon[], max: number): LatLon[] {
    if (points.length <= max) return points;
    const step = (points.length - 1) / (max - 1);
    const kept: LatLon[] = [];
    for (let k = 0; k < max; k++) kept.push(points[Math.round(k * step)]);
    return kept;
}

/**
 * One GPX body → one named line with every point.
 *
 * One, deliberately: a corridor part buffers a single polyline, and a file whose tracks
 * are two different rides belongs in the panel as two files. Multiple `<trkseg>`s are
 * joined — one ride with gaps of metres, not two rides — and `<rtept>`s count when there
 * are no `<trkpt>`s.
 *
 * @param fallbackName used when the file names nothing — the filename, usually.
 * @throws {GpxError} when no usable points survive.
 */
export function readGpx(text: string, fallbackName: string): { name: string; points: GpxPoint[]; waypoints: GpxWaypoint[] } {
    const elements = [...text.matchAll(/<(trkpt|rtept)\b([^>]*?)(?:\/>|>([\s\S]*?)<\/\1\s*>)/g)];
    const trk: GpxPoint[] = [];
    const rte: GpxPoint[] = [];
    let malformed = 0;
    for (const [, kind, attributes, body] of elements) {
        const p: GpxPoint | null = pointOf(attributes);
        if (!p) {
            malformed++;
            continue;
        }
        const ele = Number(/<ele>\s*([^<\s][^<]*?)\s*<\/ele>/.exec(body ?? "")?.[1] ?? NaN);
        if (Number.isFinite(ele)) p.ele = ele;
        (kind === "trkpt" ? trk : rte).push(p);
    }
    const points = trk.length ? trk : rte;
    if (points.length < 2) {
        throw new GpxError(
            elements.length === 0
                ? "no track or route points found — is this a GPX file?"
                : malformed > 0
                  ? `no usable points — ${malformed} of ${elements.length} carried malformed coordinates`
                  : "the file has fewer than two points, which is not a route",
        );
    }
    // A waypoint with malformed coordinates is skipped; it never refuses the route.
    const waypoints = [...text.matchAll(WPT)].flatMap(([, attributes, body]): GpxWaypoint[] => {
        const p = pointOf(attributes);
        const note = firstText(body ?? "", "desc") ?? firstText(body ?? "", "cmt");
        return p ? [{ ...p, name: firstText(body ?? "") ?? undefined, ...(note ? { note } : {}) }] : [];
    });
    return { name: nameOf(text) ?? fallbackName, points, waypoints };
}

/** One GPX body → one corridor route, decimated to {@link MAX_ROUTE_POINTS}. */
export function parseGpx(text: string, fallbackName: string): GpxRoute {
    const { name, points } = readGpx(text, fallbackName);
    const decimated = decimate(points.map(({ lat, lon }) => ({ lat, lon })), MAX_ROUTE_POINTS);
    return { name, points: decimated, distanceKm: lengthKm(decimated) };
}
