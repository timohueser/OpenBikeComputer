/**
 * Web ride export, end to end against the simulated device (C5, #904).
 *
 * Two things are being decided here, and only one of them is "does the flow work".
 *
 * The first is **byte identity**: the GPX a visitor saves has to be the file the device itself would
 * have written. The pinned pair is `specs/vectors/track-log.obct` → `track-export.gpx`, produced
 * by the real `obc_route::track_to_gpx`, and the export path has to land on those exact bytes after
 * a full round trip through the wire's ride object — with one documented exception the wire format
 * makes unavoidable, asserted as *the only* exception rather than waved at.
 *
 * The second is **that nothing happened to the device**. Under protocol v4 that claim is a structural
 * one rather than a behavioural one: §5.2.2 retires the v1 `command` selector, so there is no ack, no
 * clock write and no config write on this cable at all — the only two things a peer can do to an
 * object are a `PUT` and a `REMOVE`, and {@link rideAccess} hands the export path an object that has
 * neither, at compile time *and* at runtime.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { initConvert } from "../convert/bridge";
import { DeviceError, FlatStoreClient } from "../usb/client";
import {
    MockDevice,
    REFERENCE_STORE_ID,
    loopbackDevice,
    loopbackLink,
    type LoopbackOptions,
    type MockDeviceOptions,
} from "../usb/loopback";
import { encodeRideObject, type RideObject, type RidePoint } from "../usb/objects";
import type { BytePipe, DeviceLink } from "../usb/pipe";
import { EntryFlags, ObjectKind, type CatalogEntry } from "../usb/protocol";
import type { JobContext, JobPhase } from "./progress";
import {
    exportRide,
    recordedRides,
    rideAccess,
    rideDate,
    rideFilename,
    rideKey,
    rideScope,
    rideToTrackLog,
    scopeKey,
    storeEra,
    type RideSource,
} from "./rides";

// --- fixtures -----------------------------------------------------------------

function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "specs", "vectors", "manifest.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root from " + import.meta.url);
}

const ROOT = repoRoot();
const vector = (name: string) => new Uint8Array(readFileSync(join(ROOT, "specs/vectors", name)));

/** The name `track-export.gpx` was produced with (`obc_vectors::TRACK_NAME`). */
const TRACK_NAME = "Schauinsland & back";

beforeAll(async () => {
    const wasm = join(dirname(fileURLToPath(import.meta.url)), "..", "convert", "pkg", "obc_web_convert_bg.wasm");
    if (!existsSync(wasm)) {
        throw new Error(`the wasm bridge is not built (${wasm} missing). Run \`npm run build:wasm\`.`);
    }
    await initConvert(readFileSync(wasm));
});

const TRACK_RECORD_LEN = 20;

/**
 * The device's own Finish-time conversion, `obc_route::track_to_ride`, restated in the test.
 *
 * It is the *producer* of every ride object that ever crosses this wire, so the honest way to test
 * the pull side is to hand it exactly what the device would have sent — a ride object built from the
 * checked-in log by the same four rules (µdeg × 10, `lat, lon` order, whole-second offsets from the
 * first record, sensors 1:1) — rather than a ride object shaped to make the export pass.
 */
function rideFromTrackLog(log: Uint8Array, name: string, startTime: number): RideObject {
    const view = new DataView(log.buffer, log.byteOffset, log.byteLength);
    const total = Math.floor(log.length / TRACK_RECORD_LEN); // a trailing partial record is ignored
    const t0 = total > 0 ? view.getUint32(12, true) : 0;
    const points: RidePoint[] = [];
    for (let i = 0; i < total; i++) {
        const at = i * TRACK_RECORD_LEN;
        const hr = log[at + 16];
        const cadence = log[at + 17];
        const power = view.getUint16(at + 18, true);
        points.push({
            tOffsetS: Math.floor((view.getUint32(at + 12, true) - t0) / 1000),
            lat1e7: view.getInt32(at + 4, true) * 10,
            lon1e7: view.getInt32(at, true) * 10,
            eleM: view.getInt16(at + 8, true),
            hrBpm: hr === 0xff ? null : hr,
            cadenceRpm: cadence === 0xff ? null : cadence,
            powerW: power === 0xffff ? null : power,
        });
    }
    return {
        version: 2,
        name,
        startTime,
        distanceM: 4210,
        movingTimeS: 1_284,
        avgSpeedCms: 328,
        climbM: 118,
        avgHr: 135,
        maxHr: 138,
        avgCadence: 78,
        avgPower: 205,
        maxPower: 240,
        points,
    };
}

/** A long ride, for the paths that need a transfer still running when something goes wrong. */
function longRide(points: number): RideObject {
    const list: RidePoint[] = [];
    for (let i = 0; i < points; i++) {
        list.push({
            tOffsetS: i,
            lat1e7: 479_950_000 + i * 100,
            lon1e7: 78_420_000 + i * 100,
            eleM: 300 + (i % 200),
            hrBpm: 120 + (i % 40),
            cadenceRpm: 80,
            powerW: 200,
        });
    }
    return {
        version: 2,
        name: "Long Way Round",
        startTime: 1_783_598_400,
        distanceM: points * 8,
        movingTimeS: points,
        avgSpeedCms: 800,
        climbM: 900,
        avgHr: 140,
        maxHr: 160,
        avgCadence: 80,
        avgPower: 200,
        maxPower: 400,
        points: list,
    };
}

// --- a job context a test can watch --------------------------------------------

interface Watched extends JobContext {
    readonly phases: JobPhase[];
    readonly last: [number, number];
}

function context(options: { signal?: AbortSignal; at?: (done: number, phase: JobPhase) => void } = {}): Watched {
    const phases: JobPhase[] = [];
    let phase: JobPhase = "idle";
    let last: [number, number] = [0, 0];
    return {
        signal: options.signal ?? new AbortController().signal,
        cancel() {},
        phases,
        get last() {
            return last;
        },
        phase(next) {
            phase = next;
            phases.push(next);
        },
        progress(done, total) {
            last = [done, total];
            options.at?.(done, phase);
        },
    };
}

/** A device with seeded rides, and the read-only handle the export path gets. */
function deviceWith(
    rides: Array<{ id: bigint; ride: RideObject }>,
    options: LoopbackOptions & MockDeviceOptions = {},
) {
    const rig = loopbackDevice(options);
    const entries = rides.map(({ id, ride }) =>
        rig.device.seed({
            objectId: id,
            kind: ObjectKind.Ride,
            displayName: ride.name,
            bytes: encodeRideObject(ride),
        }),
    );
    return { ...rig, entries, source: rideAccess(rig.client) };
}

// --- byte identity --------------------------------------------------------------

describe("the exported GPX", () => {
    /**
     * The one thing the wire cannot carry.
     *
     * `track_to_gpx` opens a fresh `<trkseg>` on every point flagged `segment_start`, so the pinned
     * fixture — a log with a pause in it — has two. The ride object (§7.2) has no segment flag:
     * the device drops it at Finish, in `track_to_ride`, for *every* peer. The phone's exporter says
     * the same thing ("The ride object carries no segment breaks, so the track is one `<trkseg>`").
     *
     * Collapsing exactly that one break — and nothing else — is what "byte-identical to the native
     * path for the same track" can honestly mean on this side of the wire. Deriving the expectation
     * from the fixture rather than hand-writing it means every other byte still has to match:
     * coordinates, elevations, the sensor extension shape, the XML escaping, the name.
     */
    function withoutSegmentBreak(gpx: string): string {
        const joined = gpx.replace("</trkseg>\n<trkseg>\n", "");
        expect(joined, "the fixture is expected to have exactly one mid-track segment break").not.toBe(gpx);
        expect(joined.match(/<trkseg>/g), "and only one after collapsing it").toHaveLength(1);
        return joined;
    }

    it("reproduces the native exporter byte-for-byte, pulled from the device", async () => {
        const log = vector("track-log.obct");
        const ride = rideFromTrackLog(log, TRACK_NAME, 1_783_598_400);
        const { entries, source, close } = deviceWith([{ id: 4n, ride }]);
        try {
            // The catalog is what a rider picks from, so the export starts where they do.
            const listed = await source.listRides();
            expect(listed.map((entry) => entry.objectId)).toEqual([4n]);

            const exported = await exportRide(source, listed[0], context());
            const expected = new TextDecoder().decode(vector("track-export.gpx"));
            expect(exported.gpx).toBe(withoutSegmentBreak(expected));
            expect(exported.points).toBe(5);
            expect(BigInt(exported.bytes)).toBe(entries[0].payloadLength);
        } finally {
            await close();
        }
    });

    it("is UTF-8 bytes, so the saved file is what was compared", async () => {
        // The comparison above is on a string; what reaches a Blob is bytes. An em dash or a
        // non-ASCII ride name is the case where those two stop agreeing if anything re-encodes.
        const log = vector("track-log.obct");
        const ride = rideFromTrackLog(log, "Höhenweg — Schauinsland", 1_783_598_400);
        const { source, close } = deviceWith([{ id: 1n, ride }]);
        try {
            const exported = await exportRide(source, (await source.listRides())[0], context());
            const bytes = new TextEncoder().encode(exported.gpx);
            expect(new TextDecoder("utf-8", { fatal: true }).decode(bytes)).toBe(exported.gpx);
            expect(exported.gpx).toContain("<trk><name>Höhenweg — Schauinsland</name>");
        } finally {
            await close();
        }
    });
});

describe("the ride object -> track log transcode", () => {
    /**
     * The inverse of `track_to_ride`, pinned field by field.
     *
     * Round-tripping the checked-in log through the wire's ride object and back must return the
     * same 20-byte records — *except* the two fields the ride object provably cannot carry. Naming
     * them as byte ranges rather than asserting "close enough" means a coordinate that started
     * rounding, an elevation that picked up a sentinel, or a sensor that lost its absence would fail
     * here with the record and the offset that moved.
     */
    it("returns every field the ride object carries, and only loses the two it does not", () => {
        const log = vector("track-log.obct");
        const ride = rideFromTrackLog(log, TRACK_NAME, 1_783_598_400);
        const back = rideToTrackLog(ride);

        const records = Math.floor(log.length / TRACK_RECORD_LEN);
        expect(back.length).toBe(records * TRACK_RECORD_LEN); // the trailing partial record is gone
        for (let i = 0; i < records; i++) {
            const at = i * TRACK_RECORD_LEN;
            const original = log.subarray(at, at + TRACK_RECORD_LEN);
            const roundTripped = back.subarray(at, at + TRACK_RECORD_LEN);
            // lon, lat, ele — exact: the device multiplied µdeg by 10, so every value divides back.
            expect(roundTripped.subarray(0, 10), `record ${i} coordinates/elevation`).toEqual(
                original.subarray(0, 10),
            );
            // hr, cadence, power — exact, sentinels and all.
            expect(roundTripped.subarray(16, 20), `record ${i} sensors`).toEqual(original.subarray(16, 20));
        }

        // The two losses, stated: the segment flag is gone (the ride object has no such field), and
        // the timestamp is whole seconds since the first point rather than the device's raw ms clock.
        const flagsAt = 10;
        expect(new DataView(log.buffer, log.byteOffset).getUint16(3 * TRACK_RECORD_LEN + flagsAt, true)).toBe(1);
        for (let i = 0; i < records; i++) {
            expect(new DataView(back.buffer).getUint16(i * TRACK_RECORD_LEN + flagsAt, true)).toBe(0);
        }
        expect(new DataView(back.buffer).getUint32(3 * TRACK_RECORD_LEN + 12, true)).toBe(63_000);
    });

    it("writes the device's own 'no barometer yet' value rather than a sentinel altitude", () => {
        // The firmware never emits `ELE_NONE`; it stamps 0 until the first baro sample. Another
        // encoder can, and `<ele>-32768</ele>` in a rider's GPX would be worse than 0.
        const ride: RideObject = {
            ...longRide(1),
            points: [{ tOffsetS: 0, lat1e7: 479_950_000, lon1e7: 78_420_000, eleM: null, hrBpm: null, cadenceRpm: null, powerW: null }],
        };
        const log = rideToTrackLog(ride);
        expect(new DataView(log.buffer).getInt16(8, true)).toBe(0);
    });
});

// --- the surface the export path is given -----------------------------------------
//
// This used to be a four-test suite about the ride acknowledgement — that the browser never sends
// one, asserted against the device's command log and its `/tracks/SYNCED.SET` bytes. There is no ack
// on the cable any more: §5.2.2 retires the v1 `command` selector outright, because a possession ack
// changes no object and so has no store meaning. It keeps the BLE control surface it had; USB does
// not carry it, and neither does anything below. What survives is the narrowing itself, which is
// still load-bearing because §3.6 and §3.7 *are* on this cable.

describe("what the export path can reach", () => {
    it("hands the export path a device it cannot write to", async () => {
        const { client, source, close } = deviceWith([]);
        try {
            expect(Object.keys(source).sort()).toEqual(["downloadRide", "listRides"]);
            for (const forbidden of ["put", "remove", "arm", "cancel", "close", "get", "list"]) {
                expect(source, `\`${forbidden}\` must not be reachable from the export path`).not.toHaveProperty(
                    forbidden,
                );
            }
            // A type is a compile-time fact and a cast defeats it, so the narrowing is real at
            // runtime too: the object does not *have* the method, and the cast throws.
            expect(() => (source as unknown as FlatStoreClient).remove({ objectId: 1n, revision: 1n })).toThrow(
                TypeError,
            );
            expect(Object.isFrozen(source), "and it cannot be grown back").toBe(true);
            // The client itself still has it — this is a narrowing at the seam, not a removal.
            expect(typeof client.remove).toBe("function");
            // A `FlatStoreClient` is deliberately *not* a `RideSource` — `downloadRide` exists
            // nowhere else — so `rideAccess` is the only way to obtain one, and there is no
            // "pass the client straight in" shortcut for a call site to take.
            const notASource: Partial<RideSource> = client as unknown as Partial<RideSource>;
            expect(notASource.downloadRide).toBeUndefined();
        } finally {
            await close();
        }
    });
});

// --- honest failure --------------------------------------------------------------

describe("when the export cannot finish", () => {
    /** A host link that flips one byte on its way in, once armed — a wire error the device's
     *  declared whole-payload CRC (§3.5) has to catch before anything is handed to a rider. */
    function corruptible(link: DeviceLink): { link: DeviceLink; arm: () => void } {
        const stream = link.stream;
        let armed = false;
        let flipped = false;
        const wrapped: BytePipe = {
            transport: stream.transport,
            get open() {
                return stream.open;
            },
            async read(signal) {
                const slice = await stream.read(signal);
                if (!armed || flipped || slice.length === 0) return slice;
                flipped = true;
                const damaged = slice.slice();
                damaged[damaged.length - 1] ^= 0xff;
                return damaged;
            },
            write: (bytes, signal) => stream.write(bytes, signal),
            reset: () => stream.reset(),
            close: () => stream.close(),
        };
        return {
            link: { control: link.control, stream: wrapped, vendorIn: link.vendorIn, close: () => link.close() },
            arm: () => (armed = true),
        };
    }

    it("refuses to offer a file whose bytes did not survive the cable", async () => {
        const raw = loopbackLink();
        const device = new MockDevice(raw.device);
        void device.run();
        const wire = corruptible(raw.host);
        const client = new FlatStoreClient(wire.link);
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        device.seed({ objectId: 4n, kind: ObjectKind.Ride, displayName: ride.name, bytes: encodeRideObject(ride) });
        try {
            const source = rideAccess(client);
            const entry = (await source.listRides())[0];
            wire.arm(); // the catalog arrived intact; damage the ride itself
            const failure = await exportRide(source, entry, context()).catch((e: unknown) => e);
            expect(failure).toBeInstanceOf(DeviceError);
            expect((failure as DeviceError).code).toBe("checksum");
            expect((failure as DeviceError).message).toMatch(/checksum/i);
        } finally {
            device.stop();
            await client.close();
            await raw.device.close();
        }
    });

    it("reports an unplug mid-pull instead of leaving a spinner", async () => {
        // 30 000 points is about 540 KB on the wire — long enough that the cable can be pulled
        // while bytes are still moving, which is the state a stuck spinner comes from.
        const harness = deviceWith([{ id: 4n, ride: longRide(30_000) }], {
            packetSize: 4096,
            streamHighWaterMark: 8 * 1024,
        });
        const ctx = context({
            at: (done, phase) => {
                if (phase === "downloading" && done > 64 * 1024) void harness.link.device.close();
            },
        });
        const failure = await exportRide(harness.source, harness.entries[0], ctx).catch((e: unknown) => e);
        expect(failure).toBeInstanceOf(DeviceError);
        expect((failure as DeviceError).code).toBe("link");
        expect(ctx.phases).toEqual(["downloading"]); // it never reached the conversion
        await harness.close();
    }, 30_000);

    it("cancels mid-pull, and the next export on the same link is an ordinary one", async () => {
        const harness = deviceWith([{ id: 4n, ride: longRide(30_000) }], {
            packetSize: 4096,
            streamHighWaterMark: 8 * 1024,
        });
        const controller = new AbortController();
        try {
            // Cancelled from inside the progress callback, not after a timer: the loopback moves
            // half a megabyte in microtasks, so a wall-clock delay races the transfer it interrupts.
            const ctx = context({
                signal: controller.signal,
                at: (done) => {
                    if (done > 64 * 1024) controller.abort();
                },
            });
            await expect(exportRide(harness.source, harness.entries[0], ctx)).rejects.toMatchObject({
                code: "aborted",
            });
            // §3.8's cancel is bilateral, so the device released its transfer slot while this side
            // reset its channel — the retry is the first path again, not a repair path.
            const again = await exportRide(harness.source, harness.entries[0], context());
            expect(again.points).toBe(30_000);
        } finally {
            await harness.close();
        }
    }, 60_000);

    it("says the ride is gone when it was deleted on the device between listing and exporting", async () => {
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const { source, entries, close } = deviceWith([{ id: 4n, ride }]);
        try {
            const stale: CatalogEntry = { ...entries[0], objectId: 99n };
            const failure = await exportRide(source, stale, context()).catch((e: unknown) => e);
            expect((failure as DeviceError).code).toBe("not-found");
        } finally {
            await close();
        }
    });

    it("is refused by the device for a ride it is still recording", async () => {
        // §3.5: a `RECORDING` entry's length and CRC are zero until the commit that ends it, so
        // serving one would report success over an empty payload. `recordedRides` is the filter that
        // keeps a caller away from it — this pins that the device refuses anyway, so the filter is a
        // convenience rather than the only thing standing between a rider and an empty GPX.
        const { device, source, close } = deviceWith([]);
        try {
            const live = device.seed({ kind: ObjectKind.Ride, displayName: "In progress", flags: EntryFlags.Recording });
            const failure = await exportRide(source, live, context()).catch((e: unknown) => e);
            expect(failure).toBeInstanceOf(DeviceError);
            expect((failure as DeviceError).code).toBe("invalid-request");
        } finally {
            await close();
        }
    });

    it("tells 'this page is behind the device' from 'the transfer broke'", async () => {
        // A ride object version this build does not decode arrives with a *matching* CRC — the
        // bytes are fine, the two ends disagree about the layout. Reporting that as a transfer
        // failure would send the rider to re-plug a cable that is working.
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const { device, source, close } = deviceWith([]);
        try {
            const future = encodeRideObject(ride);
            future[0] = 3;
            device.seed({ objectId: 4n, kind: ObjectKind.Ride, displayName: ride.name, bytes: future });
            const failure = await exportRide(source, (await source.listRides())[0], context()).catch(
                (e: unknown) => e,
            );
            expect(failure).toMatchObject({ name: "RideExportError", code: "unreadable-ride" });
            expect((failure as Error).message).toMatch(/newer firmware/);
        } finally {
            await close();
        }
    });

    it("explains an empty ride rather than failing inside the converter", async () => {
        const { source, close } = deviceWith([{ id: 4n, ride: { ...longRide(1), points: [] } }]);
        try {
            const failure = await exportRide(source, (await source.listRides())[0], context()).catch(
                (e: unknown) => e,
            );
            expect(failure).toMatchObject({ name: "RideExportError", code: "empty-ride" });
            expect((failure as Error).message).toMatch(/no recorded points/);
        } finally {
            await close();
        }
    });
});

// --- what the panel shows --------------------------------------------------------

describe("listing", () => {
    it("pages the whole catalog, because §3.3 drops nothing", async () => {
        // The v1 wire capped a listing and reported `total > count`; a client's job was to surface
        // the truncation. §3.3 pages instead — the client walks the `(ObjectId, Revision)` cursor to
        // the end — so there is no truncated listing left to render, and a page size small enough to
        // force three round trips must still produce every ride.
        const harness = deviceWith([], { pageEntries: 2 });
        try {
            const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
            const bytes = encodeRideObject(ride);
            for (let id = 1n; id <= 5n; id++) {
                harness.device.seed({ objectId: id, kind: ObjectKind.Ride, displayName: ride.name, bytes });
            }
            const listed = await harness.source.listRides();
            expect(listed.map((entry) => entry.objectId)).toEqual([1n, 2n, 3n, 4n, 5n]);
        } finally {
            await harness.close();
        }
    });

    it("offers every ride except the one being recorded", async () => {
        const harness = deviceWith([]);
        try {
            const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
            harness.device.seed({ objectId: 1n, kind: ObjectKind.Ride, bytes: encodeRideObject(ride) });
            harness.device.seed({ objectId: 2n, kind: ObjectKind.Ride, flags: EntryFlags.Recording });
            const listed = await harness.source.listRides();
            expect(listed.map((entry) => entry.objectId), "the listing itself hides nothing").toEqual([1n, 2n]);
            expect(recordedRides(listed).map((entry) => entry.objectId)).toEqual([1n]);
        } finally {
            await harness.close();
        }
    });
});

describe("ride identity", () => {
    it("keys a ride by (serial, era, id), so a recycled id is a different ride", () => {
        const before = { serial: "0011223344556677", epoch: 0xa1b2c3d4 };
        const after = { serial: "0011223344556677", epoch: 0x00000001 };
        expect(rideKey(before, 4n)).not.toBe(rideKey(after, 4n));
        expect(scopeKey(before)).not.toBe(scopeKey(after));
        // The wire's `u64` and the library index's JSON number stringify alike, which is what lets
        // one key function serve both sides.
        expect(rideKey(before, 4n)).toBe(rideKey(before, 4));
        // A device with no readable card has *no* era — never era 0, which is a legal fingerprint.
        expect(scopeKey({ serial: "x", epoch: null })).not.toBe(scopeKey({ serial: "x", epoch: 0 }));
    });

    it("takes the era from the card's StoreId and the serial from §5.2.1's strings", () => {
        // 32 bits of the 128-bit `StoreId`: a cache key, never an authorisation, and never sent
        // anywhere — the desktop ride index stores it as a `u32` and both ends must agree.
        expect(storeEra(REFERENCE_STORE_ID)).toBe(0x8f2c41d9);
        const info = { firmwareRevision: "0.4.0", hardwareRevision: "obc-lm20-r1", serialNumber: "AABB" };
        expect(rideScope(info, { storeId: REFERENCE_STORE_ID, commitSequence: 3n })).toEqual({
            serial: "AABB",
            epoch: 0x8f2c41d9,
        });
        expect(rideScope(info, null)).toEqual({ serial: "AABB", epoch: null });
        expect(rideScope(null, null)).toEqual({ serial: "", epoch: null });
    });

    it("names the file by date then ride, taking the date from the payload", () => {
        // §3.3's 88-byte entry is id, revision, length, CRC, kind, flags and a display name — there
        // is no start time in it. So a caller naming a file before it has downloaded the ride gets
        // the name alone, which is the honest half rather than a fabricated day.
        const entry: CatalogEntry = {
            objectId: 4n,
            revision: 1n,
            payloadLength: 140n,
            payloadCrc32: 0,
            kind: ObjectKind.Ride,
            flags: 0,
            displayName: TRACK_NAME,
        };
        const ride = { ...longRide(1), name: TRACK_NAME, startTime: 1_783_598_400 };
        expect(rideFilename(entry)).toBe("schauinsland-back.gpx");
        expect(rideFilename(entry, ride)).toBe("2026-07-09-schauinsland-back.gpx");
        expect(rideFilename(entry, { ...ride, startTime: 0 })).toBe("schauinsland-back.gpx");
        expect(rideFilename({ ...entry, displayName: "" }, { ...ride, name: "" })).toBe("2026-07-09-ride-4.gpx");
        // UTC, not the visitor's zone: the ride object's start_time is UTC seconds, and a local
        // rendering would file a late ride under the wrong day west of Greenwich.
        expect(rideDate(1_783_598_400)).toBe("2026-07-09");
        expect(rideDate(0)).toBeNull();
    });
});

/** The panel's flow, once: list, pick, export — with the phases a progress bar renders. */
describe("the panel's flow", () => {
    it("reports downloading then converting, and ends with a file to save", async () => {
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const { source, close } = deviceWith([{ id: 4n, ride }]);
        try {
            const ctx = context();
            const listed = await source.listRides();
            const exported = await exportRide(source, listed[0], ctx);
            expect(ctx.phases).toEqual(["downloading", "converting"]);
            expect(exported.filename).toBe("2026-07-09-schauinsland-back.gpx");
            expect(exported.gpx).toContain("<gpx version=\"1.1\" creator=\"OpenBikeComputer\"");
        } finally {
            await close();
        }
    });
});
