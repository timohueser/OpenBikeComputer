/**
 * The browser side of the conversion bridge (epic #894, A2 — issue #896).
 *
 * GPX -> OBCR and recorded ride log -> GPX, run client-side by `apps/obc-web-convert`
 * compiled to wasm. There is no TypeScript re-implementation here on purpose: the bytes a
 * visitor downloads are produced by the same `obc-route` code the device and the CLI run, and
 * `bridge.test.ts` pins that equality against the checked-in `protocol-vectors/` fixtures.
 *
 * The wasm module is fetched through a **dynamic import**, so the ~95 KB of glue + module land in
 * their own bundle chunk and cost nothing until someone actually drops a file. The first call
 * pays for the fetch; every later one is a plain function call.
 *
 * Everything this module throws is a {@link ConvertError} — including a failed load or an
 * unexpected wasm trap, which arrive as `code: "internal"`. Callers never have to guess at a
 * message shape.
 */

import type { InitInput } from "./pkg/obc_web_convert.js";

/**
 * Why a conversion failed. Mirrors `ErrorCode::as_str` in
 * `apps/obc-web-convert/src/convert.rs` — the two are one contract, so add or rename in both.
 *
 * - `empty-file` — the dropped file is zero bytes.
 * - `not-gpx` — the route file is not XML at all (a `.fit`/`.tcx` export, a zip, an image).
 * - `gpx-no-track-points` — valid GPX with no `<trkpt>`: waypoints or a `<rte>` route only.
 * - `gpx-too-many-points` — past the OBCR storage ceiling even after decimation.
 * - `not-track-log` — an XML document handed to the ride-log direction (usually a GPX).
 * - `track-no-points` — the ride log is shorter than one 20-byte record.
 * - `input-truncated` — a read ran off the end of the file.
 * - `not-route` — the bytes handed to the route read-back are not an OBCR route.
 * - `internal` — a defect in the bridge, or the module failed to load. The message says so.
 */
export type ConvertErrorCode =
    | "empty-file"
    | "not-gpx"
    | "gpx-no-track-points"
    | "gpx-too-many-points"
    | "not-track-log"
    | "track-no-points"
    | "input-truncated"
    | "not-route"
    | "internal";

/** A conversion failure: a stable {@link ConvertErrorCode} plus a message written for a rider. */
export class ConvertError extends Error {
    readonly code: ConvertErrorCode;

    constructor(code: ConvertErrorCode, message: string) {
        super(message);
        this.name = "ConvertError";
        this.code = code;
    }
}

type Bridge = typeof import("./pkg/obc_web_convert.js");

/**
 * The in-flight or settled module load. Memoized so concurrent drops share one fetch; cleared on
 * failure so a transient network error can be retried rather than cached forever.
 */
let loading: Promise<Bridge> | null = null;

/**
 * Load and instantiate the wasm module, if it is not already up.
 *
 * `source` overrides where the `.wasm` comes from. Leave it out in the browser: the generated glue
 * resolves the module next to itself, which is the form the bundler rewrites to a hashed asset
 * URL. Node has no `fetch` for `file:` URLs, so tests (and any other non-browser host) pass the
 * bytes directly.
 *
 * Calling this early — say, when the drop target is first hovered — turns the first conversion
 * into a plain function call. It is optional; the convert functions load on demand.
 */
export function initConvert(source?: InitInput): Promise<void> {
    if (!loading) {
        const pending = load(source);
        loading = pending;
        // Drop the memo if it settles as a failure, so the next call retries. Attached here (not
        // in the caller) so a caller that ignores the returned promise still cannot wedge the
        // module into a permanently-failed state.
        pending.catch(() => {
            if (loading === pending) loading = null;
        });
    }
    return loading.then(() => undefined);
}

async function load(source?: InitInput): Promise<Bridge> {
    let mod: Bridge;
    try {
        mod = await import("./pkg/obc_web_convert.js");
        await mod.default(source === undefined ? undefined : { module_or_path: source });
    } catch (cause) {
        throw new ConvertError(
            "internal",
            `The conversion module could not be loaded (${describe(cause)}). Check your connection and reload the page.`,
        );
    }
    return mod;
}

/**
 * Convert a GPX file's bytes into a `.obcr` route named `name`.
 *
 * The returned array is a fresh copy owned by JS — safe to hold on to, hand to a `Blob`, or send
 * over WebUSB after further conversions have run.
 *
 * @throws {ConvertError} with an actionable message; see {@link ConvertErrorCode}.
 */
export async function gpxToObcr(gpx: Uint8Array, name: string): Promise<Uint8Array> {
    const mod = await ensure();
    try {
        return mod.obc_convert_gpx_to_obcr(gpx, name);
    } catch (cause) {
        throw asConvertError(cause);
    }
}

/**
 * Convert a recorded `.obct` ride log's bytes into a GPX 1.1 document naming the track `name`.
 *
 * @throws {ConvertError} with an actionable message; see {@link ConvertErrorCode}.
 */
export async function trackToGpx(log: Uint8Array, name: string): Promise<string> {
    const mod = await ensure();
    try {
        return mod.obc_convert_track_to_gpx(log, name);
    } catch (cause) {
        throw asConvertError(cause);
    }
}

/** One point of a decoded route polyline, degrees and metres. */
export interface TrackPoint {
    readonly lat: number;
    readonly lon: number;
    readonly ele: number;
}

/**
 * Read a `.obcr` route's polyline back out — the preview's direction. The wasm side returns flat
 * `[lat°, lon°, ele m]` triples in one `Float64Array`; this unpacks them into points.
 *
 * @throws {ConvertError} with an actionable message; see {@link ConvertErrorCode}.
 */
export async function routeTrack(obcr: Uint8Array): Promise<TrackPoint[]> {
    const mod = await ensure();
    let flat: Float64Array;
    try {
        flat = mod.obc_convert_obcr_to_track(obcr);
    } catch (cause) {
        throw asConvertError(cause);
    }
    const points: TrackPoint[] = [];
    for (let i = 0; i + 2 < flat.length; i += 3) {
        points.push({ lat: flat[i], lon: flat[i + 1], ele: flat[i + 2] });
    }
    return points;
}

function ensure(): Promise<Bridge> {
    initConvert();
    // `initConvert` always assigns before returning; the assertion just tells TypeScript so.
    return loading as Promise<Bridge>;
}

const CODES: ReadonlySet<string> = new Set<ConvertErrorCode>([
    "empty-file",
    "not-gpx",
    "gpx-no-track-points",
    "gpx-too-many-points",
    "not-track-log",
    "track-no-points",
    "input-truncated",
    "not-route",
    "internal",
]);

/**
 * Normalize whatever crossed the wasm boundary into a {@link ConvertError}.
 *
 * The Rust side throws a real `Error` carrying `code`, so the happy path is a straight read. A
 * value without a known code is a wasm trap, an out-of-memory, or a bug — reported as `internal`
 * rather than passed through, so callers only ever handle one error type.
 */
function asConvertError(cause: unknown): ConvertError {
    if (cause instanceof ConvertError) return cause;
    if (typeof cause === "object" && cause !== null) {
        const { code, message } = cause as { code?: unknown; message?: unknown };
        if (typeof code === "string" && CODES.has(code) && typeof message === "string") {
            return new ConvertError(code as ConvertErrorCode, message);
        }
    }
    return new ConvertError(
        "internal",
        `The conversion failed unexpectedly (${describe(cause)}). This is a bug — please report it with the file.`,
    );
}

function describe(cause: unknown): string {
    if (cause instanceof Error) return cause.message;
    return String(cause);
}
