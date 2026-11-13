/**
 * The drift guard between the browser conversion path and the native one.
 *
 * These exist so that a change to `obc-route` — the decimator's tolerance, the OBCR header, the GPX
 * exporter's element order — cannot ship a browser build that quietly disagrees with the device and
 * the CLI. The inputs and the expected outputs are the checked-in `specs/vectors/` fixtures, and the
 * Rust side is held to those same bytes.
 *
 * Fixture and wasm output share a source, so these prove the **wasm build agrees with the native
 * build of the same code**. They are not an independent correctness check: a bug in `gpx_to_obcr`
 * would move both together, and the converter's own correctness is tested in `obc-route`'s suite.
 *
 * Regenerate the fixtures with `cd firmware && cargo test -p obc-vectors regenerate -- --ignored`
 * after a *deliberate* change.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { ConvertError, gpxToObcr, initConvert, trackToGpx } from "./bridge";

/**
 * The OBCR header's name field, and the GPX `<trk><name>`, are inputs to the conversion, so they have
 * to match the ones the fixtures were built with. They are restated rather than imported because
 * there is no path from Rust consts into Node; that is safe precisely because they are load-bearing.
 */
const ROUTE_NAME = "Vector Loop";
const TRACK_NAME = "Schauinsland & back";

/** Walk up from this file to the repo root (the directory holding `specs/vectors/`). */
function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "specs", "vectors", "manifest.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root from " + import.meta.url);
}

const ROOT = repoRoot();
const vector = (name: string): Uint8Array => new Uint8Array(readFileSync(join(ROOT, "specs/vectors", name)));
const text = (path: string): string => readFileSync(join(ROOT, path), "utf8");

/**
 * Fail on the first differing byte with its index and both values, instead of dumping two
 * multi-hundred-byte arrays.
 */
function expectSameBytes(actual: Uint8Array, expected: Uint8Array, what: string): void {
    const n = Math.min(actual.length, expected.length);
    for (let i = 0; i < n; i++) {
        if (actual[i] !== expected[i]) {
            throw new Error(
                `${what}: first difference at byte ${i} — wasm produced 0x${actual[i].toString(16)}, ` +
                    `the native fixture has 0x${expected[i].toString(16)} (lengths ${actual.length} vs ${expected.length})`,
            );
        }
    }
    expect(actual.length, `${what}: length`).toBe(expected.length);
}

beforeAll(async () => {
    // The `--target web` glue fetches the module relative to itself and Node cannot fetch a `file:`
    // URL, so hand it the bytes. A missing artifact is a setup error, never a skip.
    const wasm = join(dirname(fileURLToPath(import.meta.url)), "..", "core", "pkg", "obc_builder_bridge_bg.wasm");
    if (!existsSync(wasm)) {
        throw new Error(
            `the wasm bridge is not built (${wasm} missing). Run \`npm run build:wasm\` in ` +
                "builder/web (needs a Rust toolchain + wasm-pack).",
        );
    }
    await initConvert(readFileSync(wasm));
});

describe("gpxToObcr", () => {
    it("reproduces the native converter's OBCR byte-for-byte, waypoints and all", async () => {
        const gpx = text("host/obc-vectors/src/route-source.gpx");
        const obcr = await gpxToObcr(new TextEncoder().encode(gpx), ROUTE_NAME, 0);
        expectSameBytes(obcr, vector("route-waypoints.obcr"), "route-waypoints.obcr");
    });

    it("reproduces the waypoint-free route byte-for-byte", async () => {
        // `obc_vectors::route_gpx_plain()`: drop every top-level `<wpt …>` line, keep the rest,
        // one trailing newline per line. Rust's `str::lines()` + push('\n') and JS's split/join
        // agree exactly as long as the source ends in a newline — asserted, not assumed.
        const source = text("host/obc-vectors/src/route-source.gpx");
        expect(source.endsWith("\n"), "route-source.gpx must end with a newline").toBe(true);
        const plain = source
            .split("\n")
            .filter((line) => !line.trimStart().startsWith("<wpt "))
            .join("\n");

        const obcr = await gpxToObcr(new TextEncoder().encode(plain), ROUTE_NAME, 0);
        expectSameBytes(obcr, vector("route-plain.obcr"), "route-plain.obcr");
    });

    it("writes the picked bike type into the header and changes nothing else", async () => {
        const gpx = new TextEncoder().encode(text("host/obc-vectors/src/route-source.gpx"));
        const road = await gpxToObcr(gpx, ROUTE_NAME, 0);
        const mtb = await gpxToObcr(gpx, ROUTE_NAME, 2);
        // Byte 7 is the OBCR header's bike type.
        expect(mtb[7]).toBe(2);
        expect(Uint8Array.from(mtb).fill(0, 7, 8)).toEqual(Uint8Array.from(road).fill(0, 7, 8));
    });

    it("returns a JS-owned copy that survives later conversions", async () => {
        const gpx = new TextEncoder().encode(text("host/obc-vectors/src/route-source.gpx"));
        const first = await gpxToObcr(gpx, ROUTE_NAME, 0);
        const snapshot = Uint8Array.from(first);
        await gpxToObcr(gpx, "Something Else", 0); // reallocates + grows wasm memory
        expect(first).toEqual(snapshot);
    });
});

describe("trackToGpx", () => {
    it("reproduces the native exporter's GPX byte-for-byte", async () => {
        const gpx = await trackToGpx(vector("ride-v6.bin"), TRACK_NAME);
        const expected = new TextDecoder().decode(vector("track-export.gpx"));
        // Compared as text so a failure diffs readably; the fixture is ASCII apart from nothing,
        // so text equality here *is* byte equality.
        expect(gpx).toBe(expected);
        expectSameBytes(new TextEncoder().encode(gpx), vector("track-export.gpx"), "track-export.gpx");
    });
});

describe("failures", () => {
    const bytes = (s: string): Uint8Array => new TextEncoder().encode(s);

    /** Run `fn`, requiring it to throw a `ConvertError`, and hand the error back for inspection. */
    async function failure(fn: () => Promise<unknown>): Promise<ConvertError> {
        try {
            await fn();
        } catch (e) {
            expect(e, "every failure is a ConvertError").toBeInstanceOf(ConvertError);
            return e as ConvertError;
        }
        throw new Error("expected a failure");
    }

    it("tells an empty file, a non-GPX file and a track-less GPX apart", async () => {
        expect((await failure(() => gpxToObcr(new Uint8Array(), "x", 0))).code).toBe("empty-file");
        expect((await failure(() => gpxToObcr(new Uint8Array([0, 1, 2, 3]), "x", 0))).code).toBe("not-gpx");
        const noTrack = await failure(() => gpxToObcr(bytes('<?xml version="1.0"?><gpx></gpx>'), "x", 0));
        expect(noTrack.code).toBe("gpx-no-track-points");
    });

    it("refuses a bike type the device does not have", async () => {
        const gpx = bytes(text("host/obc-vectors/src/route-source.gpx"));
        for (const bike of [4, -1, 258, 2.7, NaN]) {
            expect((await failure(() => gpxToObcr(gpx, "x", bike))).code, String(bike)).toBe("internal");
        }
    });

    it("tells an empty file from bytes that are not a finished ride", async () => {
        expect((await failure(() => trackToGpx(new Uint8Array(), "x"))).code).toBe("empty-file");
        expect((await failure(() => trackToGpx(bytes("<?xml?><gpx/>"), "x"))).code).toBe("not-ride");
        expect((await failure(() => trackToGpx(new Uint8Array(9), "x"))).code).toBe("not-ride");
    });

    it("rejects the retired headerless sample stream", async () => {
        expect((await failure(() => trackToGpx(vector("track-log.obct"), TRACK_NAME))).code).toBe("not-ride");
    });

    /**
     * The point of the whole error surface: "Invalid file" is not an acceptable message. Each
     * failure has to name what is wrong with *this* file and what to do next, so pin the shape —
     * a sentence long enough to be a sentence, mentioning the thing it is complaining about.
     */
    it("explains each failure instead of saying the file is invalid", async () => {
        const noTrack = await failure(() => gpxToObcr(bytes("<gpx></gpx>"), "x", 0));
        expect(noTrack.message).toContain("<trkpt>");
        expect(noTrack.message).toMatch(/re-export/i);

        const notGpx = await failure(() => gpxToObcr(new Uint8Array([0xff, 0xd8, 0xff]), "x", 0));
        expect(notGpx.message).toMatch(/\.fit|\.tcx/);

        const shortLog = await failure(() => trackToGpx(new Uint8Array(9), "x"));
        expect(shortLog.message).toContain("ride-v6");

        for (const e of [noTrack, notGpx, shortLog]) {
            expect(e.message.length, `"${e.message}" is too terse to be actionable`).toBeGreaterThan(60);
            expect(e.message).not.toMatch(/^invalid/i);
        }
    });

    it("names the storage ceiling when a route is too long", async () => {
        // A zig-zag ~2 m off its own chord at every vertex: the 1 m decimation tolerance keeps all
        // of them, so the emitter runs out of chunks. One point past the ceiling is enough.
        const parts = ['<?xml version="1.0"?><gpx><trk><trkseg>'];
        for (let i = 0; i <= 65281; i++) {
            const lon = 7_800_000 + i * 15;
            const lat = 48_000_000 + (i % 2 === 0 ? 0 : 20);
            parts.push(`<trkpt lat="${deg(lat)}" lon="${deg(lon)}"/>`);
        }
        parts.push("</trkseg></trk></gpx>");

        const e = await failure(() => gpxToObcr(bytes(parts.join("")), "Too long", 0));
        expect(e.code).toBe("gpx-too-many-points");
        expect(e.message).toContain("65281");
        expect(e.message).toMatch(/stages|density/);
    });

    /** Microdegrees to a fixed 6-decimal degree string, as the fixtures' coordinates are written. */
    function deg(udeg: number): string {
        return `${Math.trunc(udeg / 1_000_000)}.${String(udeg % 1_000_000).padStart(6, "0")}`;
    }
});
