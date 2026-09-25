/**
 * The managed ride library's pull, end to end against the simulated device.
 *
 * Each test here is a *library* assertion rather than a call-count assertion: what the library holds
 * afterwards, and whether the files behind those rows exist. The failure modes this feature has are
 * not "the wrong function was called" — they are "the app believes it has a rider's only copy of a
 * ride and it does not".
 *
 * The real durable write is Rust (`apps/obc-desktop/src/rides.rs`) and is tested there, against the
 * real filesystem, with the power cut between `write()` and `fsync()`. What *this* file owns is the
 * other half of the same claim: a ride only counts as held once the library's `import()` has
 * resolved. {@link RecordingLibrary} is a fake, not a mock of the Tauri boundary: it holds real
 * state, it can fail, and the tests read its state rather than its call log.
 *
 * A possession acknowledgement is deliberately absent from this cable surface: it changes no object,
 * so it has no store meaning and USB does not carry it. The ordering discipline it needed — an
 * import resolves only after fsync — is kept regardless, because it is what makes
 * {@link PullReport} true.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { initConvert } from "../convert/bridge";
import type { FlatStoreClient } from "../usb/client";
import { flatDevice } from "../usb/flat-device";
import { loadFlatDevice } from "../../../test-support/flat-device/load";
import { encodeRideObject, type RideObject, type RidePoint } from "../usb/objects";
import { EntryFlags, ObjectKind } from "../usb/protocol";
import {
    PREVIEW_POINTS,
    previewTrack,
    pullRide,
    pullRides,
    trackPath,
    type LibraryRide,
    type LibraryView,
    type RideImport,
    type RideLibrary,
} from "./library";
import { rideAccess, rideKey, rideScope, type RideScope } from "./rides";
import type { JobContext, JobPhase } from "./progress";

beforeAll(async () => {
    const wasm = join(dirname(fileURLToPath(import.meta.url)), "..", "convert", "pkg", "obc_web_convert_bg.wasm");
    if (!existsSync(wasm)) {
        throw new Error(`the wasm bridge is not built (${wasm} missing). Run \`npm run build:wasm\`.`);
    }
    await initConvert(readFileSync(wasm));
    await loadFlatDevice();
});

/** The loopback link's default serial, and the two cards these tests swap between. */
const SERIAL = "0011223344556677";
const CARD_A = "8f2c41d96b074ea3b1559c207de83466";
const CARD_B = CARD_A.slice(0, 8) + "00000000000000000000abcd";

function ctx(): JobContext {
    const phases: JobPhase[] = [];
    return {
        signal: new AbortController().signal,
        cancel() {},
        phase: (phase) => phases.push(phase),
        progress: () => undefined,
    };
}

/** A ride the device would have recorded: a short, real-looking track through the Black Forest. */
function rideObject(name: string, startTime: number, points = 24): RideObject {
    const track: RidePoint[] = [];
    for (let i = 0; i < points; i++) {
        track.push({
            tMs: i * 5_000,
            latMicrodegrees: Math.round((47.9 + i * 0.0012) * 1e6),
            lonMicrodegrees: Math.round((7.85 + i * 0.0009) * 1e6),
            elevationM: 300 + i * 4,
            segmentStart: i === 0,
            hrBpm: 140 + (i % 7),
            cadenceRpm: 80,
            powerW: null,
        });
    }
    return {
        version: 6,
        name,
        startTime,
        distanceM: 12_340,
        movingTimeS: 2_700,
        avgSpeedCms: 457,
        climbM: 96,
        descentM: 96,
        avgHr: 143,
        maxHr: 171,
        avgCadence: 80,
        avgPower: null,
        maxPower: null,
        energyKj: null,
        bikeType: 0,
        trip: null,
        points: track,
    };
}

/**
 * A ride library that keeps real state and can be made to fail.
 *
 * Not a mock of the `invoke()` calls — the tests below never look at what was called. It models the
 * one property the whole feature hangs off: **`import()` resolving is what makes a ride held**. A
 * rejected import leaves nothing behind, exactly as a power cut before fsync does on the real thing.
 */
class RecordingLibrary implements RideLibrary {
    readonly rides = new Map<string, LibraryRide>();
    /** Every library call, in order — read by the ordering assertions and nothing else. */
    readonly events: string[] = [];
    /** Keys whose import must fail, as a crash between write and fsync would. */
    failFor = new Set<string>();
    folder = "/library";

    async view(): Promise<LibraryView> {
        return { folder: this.folder, isDefault: true, rides: [...this.rides.values()] };
    }

    async import(ride: RideImport): Promise<{ ride: LibraryRide; imported: boolean }> {
        const key = `${ride.serial}:${ride.storeId}:${ride.objectId}`;
        if (this.failFor.has(key)) {
            this.events.push(`import-failed:${ride.objectId}`);
            throw new Error("simulated power loss between write() and fsync()");
        }
        const held = this.rides.get(key);
        if (held) {
            // The real thing rewrites the files it is missing, keeps the names and keeps
            // `importedAt`, and reports `imported: false` — this is the same ride arriving again,
            // not a new one. Mirrored here so the pull's repair path is exercised rather than
            // stubbed.
            const repaired = { ...held, present: true, gpxPresent: true };
            this.rides.set(key, repaired);
            this.gpx.set(key, ride.gpx);
            this.objects.set(key, ride.object);
            this.events.push(held.present ? `import-noop:${ride.objectId}` : `repaired:${ride.objectId}`);
            return { ride: repaired, imported: false };
        }
        const landed: LibraryRide = {
            key,
            serial: ride.serial,
            storeId: ride.storeId,
            objectId: ride.objectId,
            name: ride.name,
            startTime: ride.startTime,
            distanceM: ride.distanceM,
            movingTimeS: ride.movingTimeS,
            climbM: ride.climbM,
            points: ride.points,
            bytes: ride.object.length,
            crc32: ride.crc32,
            importedAt: 1_700_000_000 + this.rides.size,
            // The split layout: the archive lives in app data, only the GPX in the visible folder.
            ridePath: `/app-data/ride-archive/${ride.objectId}.obcride`,
            gpxPath: `${this.folder}/${ride.objectId}.gpx`,
            track: ride.track,
            present: true,
            gpxPresent: true,
        };
        this.rides.set(key, landed);
        this.gpx.set(key, ride.gpx);
        this.objects.set(key, ride.object);
        this.events.push(`imported:${ride.objectId}`);
        return { ride: landed, imported: true };
    }

    readonly gpx = new Map<string, string>();
    readonly objects = new Map<string, Uint8Array>();

    async readObject(key: string): Promise<Uint8Array> {
        const bytes = this.objects.get(key);
        if (!bytes) throw new Error(`no ride ${key}`);
        return bytes;
    }

    async writeGpx(key: string, gpx: string): Promise<string> {
        this.gpx.set(key, gpx);
        return `${this.folder}/${key}.gpx`;
    }

    async reveal(): Promise<void> {}
    async chooseFolder(): Promise<string | null> {
        return null;
    }

    /** A ride the rider deleted in the file manager: the record stays, the bytes are gone. */
    loseFile(key: string): void {
        const held = this.rides.get(key);
        if (held) this.rides.set(key, { ...held, present: false });
    }

    /** Keys whose file is really there — "what is on the disk", which is what a pull asks. */
    held(): string[] {
        return [...this.rides.values()].filter((ride) => ride.present).map((ride) => ride.key).sort();
    }
}

/** A live client + device pair over the loopback pipe, plus the narrowed source the pull is given. */
function connect(options: { storeId?: string } = {}) {
    const rig = flatDevice({ storeId: options.storeId ?? CARD_A });
    return { ...rig, source: rideAccess(rig.client) };
}

/**
 * The connected device's `(serial, era)`, derived exactly as the UI derives it — over the two reads
 * every connection already does. Re-read rather than remembered, because that is the point of the
 * era: a card swap changes it under a running app.
 */
async function scopeNow(client: FlatStoreClient): Promise<RideScope> {
    const page = await client.listPage({});
    return rideScope(await client.deviceInfo(), { storeId: page.storeId, commitSequence: page.commitSequence });
}

/**
 * Put a ride on the device's card, catalog entry and all.
 *
 * The store assigns the id, so a caller that needs one reads it off the entry that comes back.
 * Seeding in order gives 1, 2, 3 — which is what the keys below are written against.
 */
function seedRide(device: ReturnType<typeof connect>["device"], name: string, startTime: number) {
    const bytes = encodeRideObject(rideObject(name, startTime));
    const entry = device.seed({ kind: ObjectKind.Ride, displayName: name, bytes });
    return { entry, bytes };
}

// --- acceptance ----------------------------------------------------------------

describe("pulling rides into the library", () => {
    it("pulls every ride the card holds and deduplicates them by the id the card assigned", async () => {
        // The ids are the store's, not this test's. That a key survives an id above 2^53 is pinned
        // where the key is built (`rides.test.ts`, "keys the complete card identity and decimal u64
        // object id"); what is pinned here is that a pull uses that key and repeats nothing.
        const { device, client, source, close } = connect();
        try {
            const scope = await scopeNow(client);
            const seeded = ["Dawn", "Noon", "Dusk"].map((name) => seedRide(device, name, 1_700_000_000).entry);
            const ids = seeded.map((entry) => entry.objectId);
            const library = new RecordingLibrary();
            const first = await pullRides(source, library, scope, ctx());
            expect(first.failed).toEqual([]);
            expect(first.imported.map((ride) => ride.objectId)).toEqual(ids);
            expect(library.held()).toEqual(ids.map((id) => `${SERIAL}:${CARD_A}:${id}`).sort());
            const second = await pullRides(source, library, scope, ctx());
            expect(second.imported).toEqual([]);
            expect(second.alreadyHeld).toBe(3);
        } finally {
            await close();
        }
    });

    it("is idempotent: a second pull downloads nothing, duplicates nothing and re-stamps nothing", async () => {
        const { device, client, source, close } = connect();
        try {
            const scope = await scopeNow(client);
            seedRide(device, "Dawn Patrol", 1_700_000_000);
            seedRide(device, "Gravel Hour", 1_700_100_000);
            const library = new RecordingLibrary();

            const first = await pullRides(source, library, scope, ctx());
            expect(first.listed).toBe(2);
            expect(first.imported.map((ride) => ride.objectId)).toEqual([1n, 2n]);
            const stamps = [...library.rides.values()].map((r) => [r.key, r.importedAt] as const);

            const second = await pullRides(source, library, scope, ctx());
            expect(second.imported).toEqual([]);
            expect(second.alreadyHeld).toBe(2);
            expect(library.rides.size).toBe(2);
            // The ride files were never touched a second time.
            expect(library.events.filter((e) => e.startsWith("imported:"))).toEqual(["imported:1", "imported:2"]);
            expect([...library.rides.values()].map((r) => [r.key, r.importedAt] as const)).toEqual(stamps);
        } finally {
            await close();
        }
    });

    it("skips a ride the device is still recording, and names it in the report", async () => {
        // A `GET` of a `RECORDING` entry is refused — its length and CRC are zero until the commit
        // that ends it — so there is nothing to copy yet. Counting it is what lets the page say "one
        // ride is still being recorded" rather than silently listing fewer rides than the device has.
        const { device, client, source, close } = connect();
        try {
            const scope = await scopeNow(client);
            seedRide(device, "Finished", 1_700_000_000);
            device.seed({ kind: ObjectKind.Ride, displayName: "Still going", flags: EntryFlags.Recording });
            const library = new RecordingLibrary();

            const report = await pullRides(source, library, scope, ctx());

            expect(report.recording).toBe(1);
            expect(report.listed, "the recording ride is not something the pull could act on").toBe(1);
            expect(report.imported.map((ride) => ride.objectId)).toEqual([1n]);
            expect(report.failed).toEqual([]);
            expect(library.held()).toEqual([rideKey(scope, 1n)]);
        } finally {
            await close();
        }
    });

    it("leaves a ride out of the library when its durable write fails, and fetches it again next time", async () => {
        const { device, client, source, close } = connect();
        try {
            const scope = await scopeNow(client);
            seedRide(device, "Landed", 1_700_000_000);
            seedRide(device, "Interrupted", 1_700_100_000);
            seedRide(device, "Also landed", 1_700_200_000);
            const library = new RecordingLibrary();
            library.failFor.add(rideKey(scope, 2n));

            const report = await pullRides(source, library, scope, ctx());

            expect(report.failed.map((f) => f.objectId)).toEqual([2n]);
            expect(report.imported.map((r) => r.objectId)).toEqual([1n, 3n]);
            expect(library.held()).toEqual([rideKey(scope, 1n), rideKey(scope, 3n)].sort());

            // …and the next pull, with the write working, fetches ride 2 and only then holds it.
            library.failFor.clear();
            const retry = await pullRides(source, library, scope, ctx());
            expect(retry.imported.map((r) => r.objectId)).toEqual([2n]);
            expect(library.held()).toHaveLength(3);
        } finally {
            await close();
        }
    });

    it("imports both rides when a card swap recycles an object id", async () => {
        // A re-initialized card mints a new `StoreId` and starts its ids at 1 again, so a *different*
        // ride lands on id 1. A bare-id library would hold one row and answer "up to date" forever.
        const library = new RecordingLibrary();

        const first = connect({ storeId: CARD_A });
        const oldEra = await scopeNow(first.client);
        expect(oldEra.storeId).toBe(CARD_A);
        seedRide(first.device, "Old era ride", 1_700_000_000);
        await pullRides(first.source, library, oldEra, ctx());
        expect(library.rides.size).toBe(1);
        await first.close();

        const second = connect({ storeId: CARD_B });
        try {
            const newEra = await scopeNow(second.client);
            expect(newEra.storeId).toBe(CARD_B);
            expect(newEra.serial).toBe(oldEra.serial);
            seedRide(second.device, "New era ride", 1_800_000_000);

            const after = await pullRides(second.source, library, newEra, ctx());
            expect(after.imported.map((r) => r.name)).toEqual(["New era ride"]);
            // Two rows, two keys, one object id.
            expect([...library.rides.keys()].sort()).toEqual(
                [`${SERIAL}:${CARD_A}:1`, `${SERIAL}:${CARD_B}:1`].sort(),
            );
            expect(new Set([...library.rides.values()].map((r) => r.objectId))).toEqual(new Set([1n]));
        } finally {
            await second.close();
        }
    });

    it("re-downloads a ride whose file the rider deleted, keeping its original importedAt", async () => {
        // "Do I already have it" is a question about the disk, not about the index: a record whose
        // file is gone is not a durable copy, so the ride is fetched again — and it is a repair
        // rather than an import, because the rider first got it long ago.
        const { device, client, source, close } = connect();
        try {
            const scope = await scopeNow(client);
            for (const [at, name] of ["One", "Two", "Three"].entries()) {
                seedRide(device, name, 1_700_000_000 + at * 1000);
            }
            const library = new RecordingLibrary();
            await pullRides(source, library, scope, ctx());
            const firstStamp = library.rides.get(rideKey(scope, 2n))!.importedAt;

            library.loseFile(rideKey(scope, 2n));
            const again = await pullRides(source, library, scope, ctx());

            expect(again.imported).toEqual([]);
            expect(again.repaired.map((r) => r.objectId)).toEqual([2n]);
            expect(again.repaired[0].importedAt).toBe(firstStamp);
            expect(again.alreadyHeld).toBe(2);
            expect(library.held()).toHaveLength(3);
        } finally {
            await close();
        }
    });

    it("refuses to import anything from a device whose card identity it could not read", async () => {
        // The fail-closed posture: without both halves of the era, an id from this device cannot be
        // told apart from another device's, so nothing is keyed and nothing is copied. A missing
        // StoreId never shares an identity with a readable card.
        const { device, source, close } = connect();
        try {
            seedRide(device, "Unkeyable", 1_700_000_000);
            const library = new RecordingLibrary();
            await expect(pullRides(source, library, { serial: SERIAL, storeId: null }, ctx())).rejects.toMatchObject({
                name: "RideLibraryError",
                code: "no-scope",
            });
            await expect(pullRides(source, library, { serial: "", storeId: CARD_A }, ctx())).rejects.toThrow(
                /card identity/,
            );
            expect(library.rides.size).toBe(0);
        } finally {
            await close();
        }
    });

    it("stores a GPX the device itself would have written, and the object it sent", async () => {
        const { device, client, source, close } = connect();
        try {
            const scope = await scopeNow(client);
            const { bytes } = seedRide(device, "Schauinsland", 1_700_000_000);
            const library = new RecordingLibrary();
            await pullRides(source, library, scope, ctx());

            const key = rideKey(scope, 1n);
            // The archive is the device's bytes, unaltered — which is what makes the library
            // lossless while `track_to_gpx` still omits `<time>`.
            expect(library.objects.get(key)).toEqual(bytes);
            const gpx = library.gpx.get(key) ?? "";
            expect(gpx).toContain('<gpx version="1.1" creator="OpenBikeComputer"');
            expect(gpx).toContain("<name>Schauinsland</name>");
            expect(gpx.match(/<trkpt /g) ?? []).toHaveLength(24);
        } finally {
            await close();
        }
    });

    it("takes every figure but the id and the name from the payload, because the entry has none", async () => {
        // An 88-byte catalog entry is id, revision, length, CRC, kind, flags and a display name. A
        // ride's start time, distance and moving time exist only inside the object — which this path
        // downloads anyway, so nothing is lost; what is gone is showing those figures beforehand.
        const { device, client, source, close } = connect();
        try {
            const scope = await scopeNow(client);
            const ride = rideObject("Kaiserstuhl", 1_700_300_000);
            const seeded = device.seed({
                kind: ObjectKind.Ride,
                displayName: "Kaiserstuhl",
                bytes: encodeRideObject(ride),
            });
            const library = new RecordingLibrary();

            const landed = (await pullRides(source, library, scope, ctx())).imported[0];

            expect(landed).toMatchObject({
                objectId: seeded.objectId,
                serial: scope.serial,
                storeId: scope.storeId,
                name: ride.name,
                startTime: ride.startTime,
                distanceM: ride.distanceM,
                movingTimeS: ride.movingTimeS,
                climbM: ride.climbM,
                points: ride.points.length,
            });
        } finally {
            await close();
        }
    });

    it("pulls one ride down the same path the batch uses", async () => {
        const { device, client, source, close } = connect();
        try {
            const scope = await scopeNow(client);
            seedRide(device, "First", 1_700_000_000);
            seedRide(device, "Second", 1_700_100_000);
            const library = new RecordingLibrary();

            const listed = await source.listRides();
            const one = await pullRide(source, library, scope, listed[1], ctx());

            expect(one.imported).toBe(true);
            expect(library.held()).toEqual([rideKey(scope, 2n)]);
        } finally {
            await close();
        }
    });
});

describe("the track preview", () => {
    it("is drawn from the ride's own points, keeping the first and the last", () => {
        const ride = rideObject("Long", 1_700_000_000, 5_000);
        const track = previewTrack(ride);
        expect(track.length).toBeLessThanOrEqual(PREVIEW_POINTS);
        expect(track.length).toBeGreaterThan(2);
        expect(track[0]).toEqual([
            ride.points[0].latMicrodegrees / 1e6,
            ride.points[0].lonMicrodegrees / 1e6,
        ]);
        const last = ride.points[ride.points.length - 1];
        expect(track[track.length - 1]).toEqual([
            last.latMicrodegrees / 1e6,
            last.lonMicrodegrees / 1e6,
        ]);
    });

    it("keeps a short ride whole", () => {
        expect(previewTrack(rideObject("Short", 1, 10))).toHaveLength(10);
        expect(previewTrack({ ...rideObject("None", 1, 1), points: [] })).toEqual([]);
    });

    it("fits inside its box and never divides by zero on a ride that never moved", () => {
        const path = trackPath(previewTrack(rideObject("Loop", 1, 100)), 120, 60);
        expect(path).toMatch(/^M[\d.]+ [\d.]+L/);
        const coords = [...(path ?? "").matchAll(/([\d.]+) ([\d.]+)/g)];
        expect(coords.length).toBeGreaterThan(2);
        for (const [, x, y] of coords) {
            expect(Number(x)).toBeGreaterThanOrEqual(0);
            expect(Number(x)).toBeLessThanOrEqual(120);
            expect(Number(y)).toBeGreaterThanOrEqual(0);
            expect(Number(y)).toBeLessThanOrEqual(60);
        }
        // A stationary ride is a dot, not a NaN.
        expect(trackPath([[48, 7.8], [48, 7.8]], 100, 50)).not.toContain("NaN");
        expect(trackPath([[48, 7.8]], 100, 50)).toBeNull();
        expect(trackPath([], 100, 50)).toBeNull();
    });
});
