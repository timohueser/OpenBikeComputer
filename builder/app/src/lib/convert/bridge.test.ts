/**
 * The drift guard between the browser conversion path and the native one.
 *
 * These are not "does the wrapper work" tests. They exist so that a change to `obc-route` — the
 * decimator's tolerance, the OBCR header, the GPX exporter's element order — cannot ship a
 * browser build that quietly disagrees with the device and the CLI. The inputs and the expected
 * outputs are the checked-in `specs/vectors/` fixtures, and `host/obc-vectors/tests/
 * vectors.rs` holds the Rust side to those same bytes.
 *
 * **What that proves, precisely.** The route and track fixtures are produced by running the real
 * `gpx_to_obcr` / `track_to_gpx` (the documented exception in `obc-vectors`' module header — a
 * serialization with no spec to rebuild from). So fixture and wasm output share a source, and
 * these tests prove the **wasm build agrees with the native build of the same code** — a drift
 * and miscompilation guard across the bindgen/adapter seam. They are not an independent
 * correctness check: a bug in `gpx_to_obcr` would move both together. That is the right scope
 * here — this PR adds a second *implementation host*, not a second implementation — and the
 * converter's own correctness is tested where it lives, in `obc-route`'s suite.
 *
 * Regenerate the fixtures with `cd firmware && cargo test -p obc-vectors regenerate -- --ignored`
 * after a *deliberate* change, and expect the iOS suite to want the same look.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { ConvertError, gpxToObcr, initConvert, routeTrack, routeWaypoints, trackToGpx } from "./bridge";

/**
 * The OBCR header's name field, and the GPX `<trk><name>`, are inputs to the conversion — so they
 * have to match the ones the fixtures were built with (`obc_vectors::ROUTE_NAME` / `TRACK_NAME`).
 * They are restated rather than imported because there is no path from Rust consts into Node; that
 * is safe precisely because they are load-bearing: get one wrong and the byte comparison fails.
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
 * multi-hundred-byte arrays. A byte-identity failure is usually one field, and the index says which.
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
    // `--target web` glue resolves the module relative to itself and fetches it; Node cannot fetch
    // a `file:` URL, so hand it the bytes. A missing artifact is a setup error, never a skip — a
    // silently-skipped drift guard is worse than no drift guard.
    const wasm = join(dirname(fileURLToPath(import.meta.url)), "pkg", "obc_web_convert_bg.wasm");
    if (!existsSync(wasm)) {
        throw new Error(
            `the wasm bridge is not built (${wasm} missing). Run \`npm run build:wasm\` in ` +
                "builder/app (needs a Rust toolchain + wasm-pack).",
        );
    }
    await initConvert(readFileSync(wasm));
});

describe("gpxToObcr", () => {
    it("reproduces the native converter's OBCR byte-for-byte, waypoints and all", async () => {
        const gpx = text("host/obc-vectors/src/route-source.gpx");
        const obcr = await gpxToObcr(new TextEncoder().encode(gpx), ROUTE_NAME);
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

        const obcr = await gpxToObcr(new TextEncoder().encode(plain), ROUTE_NAME);
        expectSameBytes(obcr, vector("route-plain.obcr"), "route-plain.obcr");
    });

    it("returns a JS-owned copy that survives later conversions", async () => {
        const gpx = new TextEncoder().encode(text("host/obc-vectors/src/route-source.gpx"));
        const first = await gpxToObcr(gpx, ROUTE_NAME);
        const snapshot = Uint8Array.from(first);
        await gpxToObcr(gpx, "Something Else"); // reallocates + grows wasm memory
        expect(first).toEqual(snapshot);
    });
});

describe("trackToGpx", () => {
    it("reproduces the native exporter's GPX byte-for-byte", async () => {
        const gpx = await trackToGpx(vector("ride-v3.bin"), TRACK_NAME);
        const expected = new TextDecoder().decode(vector("track-export.gpx"));
        // Compared as text so a failure diffs readably; the fixture is ASCII apart from nothing,
        // so text equality here *is* byte equality.
        expect(gpx).toBe(expected);
        expectSameBytes(new TextEncoder().encode(gpx), vector("track-export.gpx"), "track-export.gpx");
    });
});

describe("routeTrack", () => {
    it("reads back the polyline the converter stored, elevation included", async () => {
        // The fixture's OBCR is byte-pinned above, so the read-back is checked against the same
        // source of truth: what the GPX said, within the format's microdegree resolution.
        const points = await routeTrack(vector("route-waypoints.obcr"));
        expect(points.length).toBeGreaterThan(2);
        for (const p of points) {
            expect(p.lat).toBeGreaterThan(-90);
            expect(p.lat).toBeLessThan(90);
            expect(p.lon).toBeGreaterThan(-180);
            expect(p.lon).toBeLessThan(180);
            expect(Number.isFinite(p.ele)).toBe(true);
        }
        // Round-trip a fresh conversion: a stored point count equals the read-back count.
        const gpx = text("host/obc-vectors/src/route-source.gpx");
        const obcr = await gpxToObcr(new TextEncoder().encode(gpx), ROUTE_NAME);
        const view = new DataView(obcr.buffer, obcr.byteOffset, obcr.byteLength);
        expect((await routeTrack(obcr)).length).toBe(view.getUint32(32, true));
    });

    it("refuses bytes that are not a route, with the stable code", async () => {
        const failure = await routeTrack(new Uint8Array(200)).catch((e: unknown) => e);
        expect(failure).toBeInstanceOf(ConvertError);
        expect((failure as ConvertError).code).toBe("not-route");
    });
});

describe("routeWaypoints", () => {
    it("reads back the fixture's waypoint table, decoded and in route order", async () => {
        // `route-source.gpx` carries two `<wpt>`s: "Pass Summit" (`<type>Viewpoint</type>` —
        // deliberately unmapped, so generic) and "Brunnen" (`<sym>Drinking Water</sym>` → water).
        const wps = await routeWaypoints(vector("route-waypoints.obcr"));
        expect(wps.map((w) => w.name).sort()).toEqual(["Brunnen", "Pass Summit"]);
        for (let i = 1; i < wps.length; i++) {
            expect(wps[i].distAlongM).toBeGreaterThanOrEqual(wps[i - 1].distAlongM);
        }

        const brunnen = wps.find((w) => w.name === "Brunnen")!;
        expect(brunnen.category).toBe(1);
        expect(brunnen.ele).toBe(238);
        expect(brunnen.lat).toBeCloseTo(48.0001, 4);
        expect(brunnen.lon).toBeCloseTo(7.8201, 4);

        const summit = wps.find((w) => w.name === "Pass Summit")!;
        expect(summit.category).toBe(0);
        expect(summit.ele).toBeNull();

        // The absolute distances, not just their order — the modal's `km x.y` labels rest on
        // this field. The fixture is byte-pinned, so these are exact stored `uint32` metres:
        // Brunnen anchors at the first raw track point, the summit ~1.7 km in.
        expect(brunnen.distAlongM).toBe(0);
        expect(summit.distAlongM).toBe(1700);
    });

    it("agrees with a fresh conversion of the same GPX", async () => {
        // The other way the device gets waypoints: `gpxToObcr` on a GPX with `<wpt>`s. The
        // fixture is byte-pinned to that conversion above, so the tables must agree exactly.
        const gpx = text("host/obc-vectors/src/route-source.gpx");
        const obcr = await gpxToObcr(new TextEncoder().encode(gpx), ROUTE_NAME);
        expect(await routeWaypoints(obcr)).toEqual(await routeWaypoints(vector("route-waypoints.obcr")));
    });

    it("resolves to an empty list for a route without waypoints", async () => {
        expect(await routeWaypoints(vector("route-plain.obcr"))).toEqual([]);
    });

    it("refuses bytes that are not a route, with the stable code", async () => {
        const failure = await routeWaypoints(new Uint8Array(200)).catch((e: unknown) => e);
        expect(failure).toBeInstanceOf(ConvertError);
        expect((failure as ConvertError).code).toBe("not-route");
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
        expect((await failure(() => gpxToObcr(new Uint8Array(), "x"))).code).toBe("empty-file");
        expect((await failure(() => gpxToObcr(new Uint8Array([0, 1, 2, 3]), "x"))).code).toBe("not-gpx");
        const noTrack = await failure(() => gpxToObcr(bytes('<?xml version="1.0"?><gpx></gpx>'), "x"));
        expect(noTrack.code).toBe("gpx-no-track-points");
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
        const noTrack = await failure(() => gpxToObcr(bytes("<gpx></gpx>"), "x"));
        expect(noTrack.message).toContain("<trkpt>");
        expect(noTrack.message).toMatch(/re-export/i);

        const notGpx = await failure(() => gpxToObcr(new Uint8Array([0xff, 0xd8, 0xff]), "x"));
        expect(notGpx.message).toMatch(/\.fit|\.tcx/);

        const shortLog = await failure(() => trackToGpx(new Uint8Array(9), "x"));
        expect(shortLog.message).toContain("ride-v3");

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

        const e = await failure(() => gpxToObcr(bytes(parts.join("")), "Too long"));
        expect(e.code).toBe("gpx-too-many-points");
        expect(e.message).toContain("65281");
        expect(e.message).toMatch(/stages|density/);
    });

    /** Microdegrees to a fixed 6-decimal degree string, as the fixtures' coordinates are written. */
    function deg(udeg: number): string {
        return `${Math.trunc(udeg / 1_000_000)}.${String(udeg % 1_000_000).padStart(6, "0")}`;
    }
});
