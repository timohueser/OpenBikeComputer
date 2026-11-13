/**
 * The browser side of the conversion bridge.
 *
 * GPX to OBCR and finished ride to GPX, run client-side by `builder/wasm` compiled to wasm.
 * There is no TypeScript re-implementation here on purpose: the bytes a visitor downloads are
 * produced by the same `obc-route` code the device and the CLI run, and `bridge.test.ts` pins that
 * equality against the checked-in `specs/vectors/` fixtures.
 *
 * The host initializes the shared builder core before loading the app.
 *
 * Everything this module throws is a {@link ConvertError}, including a failed load or an unexpected
 * wasm trap, which arrive as `code: "internal"`.
 */

import type { InitInput } from "../core/pkg/obc_builder_bridge.js";
import { initCore } from "../core/bridge";

/**
 * Why a conversion failed. Mirrors `ErrorCode::as_str` in `builder/wasm/src/convert.rs` —
 * the two are one contract, so add or rename in both.
 *
 * - `empty-file` — the dropped file is zero bytes.
 * - `not-gpx` — the route file is not XML at all.
 * - `gpx-no-track-points` — valid GPX with no `<trkpt>`: waypoints or a `<rte>` route only.
 * - `gpx-too-many-points` — past the OBCR storage ceiling even after decimation.
 * - `not-ride` — the bytes are not one complete ride object.
 * - `ride-no-points` — the finished ride has no recorded samples.
 * - `input-truncated` — a read ran off the end of the file.
 */
export type ConvertErrorCode =
    | "empty-file"
    | "not-gpx"
    | "gpx-no-track-points"
    | "gpx-too-many-points"
    | "not-ride"
    | "ride-no-points"
    | "input-truncated"
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

type Bridge = typeof import("../core/pkg/obc_builder_bridge.js");

/**
 * Load and instantiate the wasm module, if it is not already up.
 *
 * `source` overrides where the `.wasm` comes from. Leave it out in the browser: the generated glue
 * resolves the module next to itself, which is the form the bundler rewrites to a hashed asset URL.
 * Node has no `fetch` for `file:` URLs, so tests pass the bytes directly.
 *
 */
export function initConvert(source?: InitInput): Promise<void> {
    return load(source).then(() => undefined);
}

async function load(source?: InitInput): Promise<Bridge> {
    let mod: Bridge;
    try {
        mod = await initCore(source);
    } catch (cause) {
        throw new ConvertError(
            "internal",
            `The conversion module could not be loaded (${describe(cause)}). Check your connection and reload the page.`,
        );
    }
    return mod;
}

/**
 * Convert a GPX file's bytes into a `.obcr` route named `name` and typed `bike` (`0..=3`: Road,
 * Gravel, MTB, Touring).
 *
 * The returned array is a fresh copy owned by JS — safe to hold on to, hand to a `Blob`, or send
 * over WebUSB after further conversions have run.
 *
 * @throws {ConvertError} with an actionable message; see {@link ConvertErrorCode}.
 */
export async function gpxToObcr(gpx: Uint8Array, name: string, bike: number): Promise<Uint8Array> {
    const mod = await ensure();
    try {
        return mod.obc_convert_gpx_to_obcr(gpx, name, bike);
    } catch (cause) {
        throw asConvertError(cause);
    }
}

/**
 * Convert a finished ride-v6 object's bytes into a GPX 1.1 document naming the track `name`.
 *
 * @throws {ConvertError} with an actionable message; see {@link ConvertErrorCode}.
 */
export async function trackToGpx(ride: Uint8Array, name: string): Promise<string> {
    const mod = await ensure();
    try {
        return mod.obc_convert_track_to_gpx(ride, name);
    } catch (cause) {
        throw asConvertError(cause);
    }
}

function ensure(): Promise<Bridge> {
    return load();
}

const CODES: ReadonlySet<string> = new Set<ConvertErrorCode>([
    "empty-file",
    "not-gpx",
    "gpx-no-track-points",
    "gpx-too-many-points",
    "not-ride",
    "ride-no-points",
    "input-truncated",
    "internal",
]);

/**
 * Normalize whatever crossed the wasm boundary into a {@link ConvertError}. A value without a known
 * code is a wasm trap, an out-of-memory or a bug, reported as `internal` so that callers only ever
 * handle one error type.
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
