/**
 * The managed ride library's pull, end to end against the simulated device (E2, #912).
 *
 * The four acceptance criteria of #912 are four `it(...)` blocks here, and each one is a *device*
 * assertion rather than a call-count assertion: what the device's synced set and `/tracks/SYNCED.SET`
 * bytes look like afterwards. That is deliberate, because the failure modes this feature has are
 * not "the wrong function was called" — they are "the device believes a ride is safe and it isn't".
 *
 * ## Where the fsync lives, and what this file can and cannot prove
 *
 * The real durable write is Rust (`firmware/obc-desktop/src/rides.rs`), and it is tested there,
 * against the real filesystem, with the power cut between `write()` and `fsync()`
 * (`a_crash_between_write_and_fsync_leaves_the_ride_unacked`). Node cannot run that code.
 *
 * What *this* file owns is the other half of the same claim: that the ack is sequenced after the
 * library's `import()` resolves and is computed from what the library says is on the disk — so a
 * library that fails, or that comes back from a crash without the ride, produces no flag on the
 * device. {@link RecordingLibrary} is a fake, not a mock of the Tauri boundary: it holds real
 * state, it can fail, and the tests read its state rather than its call log. The one thing it
 * models about durability is the thing that matters — a ride that was not durably written is not in
 * `durableIds()`.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { initConvert } from "../convert/bridge";
import { ProtocolClient } from "../usb/client";
import { MockDevice, loopbackLink } from "../usb/loopback";
import { encodeRideObject, type RideListEntry, type RideObject, type RidePoint } from "../usb/objects";
import { Command } from "../usb/protocol";
import {
    PREVIEW_POINTS,
    previewTrack,
    pullRides,
    rideSyncAccess,
    trackPath,
    type LibraryRide,
    type LibraryView,
    type RideImport,
    type RideLibrary,
    type RideSyncSource,
} from "./library";
import { rideKey, rideScope, type RideScope } from "./rides";
import type { JobContext, JobPhase } from "./progress";

// --- scaffolding ---------------------------------------------------------------

beforeAll(async () => {
    const wasm = join(dirname(fileURLToPath(import.meta.url)), "..", "convert", "pkg", "obc_web_convert_bg.wasm");
    if (!existsSync(wasm)) {
        throw new Error(`the wasm bridge is not built (${wasm} missing). Run \`npm run build:wasm\`.`);
    }
    await initConvert(readFileSync(wasm));
});

const SERIAL = "0011223344556677";
const EPOCH_1 = 0xa1b2c3d4;
const EPOCH_2 = 0x0f0f0f0f;

function ctx(): JobContext {
    const phases: JobPhase[] = [];
    return {
        signal: new AbortController().signal,
        phase: (phase) => phases.push(phase),
        progress: () => undefined,
    };
}

/** A ride the device would have recorded: a short, real-looking track through the Black Forest. */
function rideObject(name: string, startTime: number, points = 24): RideObject {
    const track: RidePoint[] = [];
    for (let i = 0; i < points; i++) {
        track.push({
            tOffsetS: i * 5,
            // The device writes µdeg × 10, so every value it emits is a multiple of 10 (§7.2).
            lat1e7: Math.round((47.9 + i * 0.0012) * 1e6) * 10,
            lon1e7: Math.round((7.85 + i * 0.0009) * 1e6) * 10,
            eleM: 300 + i * 4,
            hrBpm: 140 + (i % 7),
            cadenceRpm: 80,
            powerW: null,
        });
    }
    return {
        version: 2,
        name,
        startTime,
        distanceM: 12_340,
        movingTimeS: 2_700,
        avgSpeedCms: 457,
        climbM: 96,
        avgHr: 143,
        maxHr: 171,
        avgCadence: 80,
        avgPower: null,
        maxPower: null,
        points: track,
    };
}

function listEntry(objectId: number, ride: RideObject, bytes: Uint8Array): RideListEntry {
    return {
        objectId,
        byteLen: bytes.length,
        startTime: ride.startTime,
        distanceM: ride.distanceM,
        movingTimeS: ride.movingTimeS,
        avgSpeedCms: ride.avgSpeedCms,
        climbM: ride.climbM,
        name: ride.name,
    };
}

/** Put a ride on the simulated device's card, catalog entry and all. */
function seedRide(device: MockDevice, objectId: number, name: string, startTime: number): Uint8Array {
    const ride = rideObject(name, startTime);
    const bytes = encodeRideObject(ride);
    device.seedRide(listEntry(objectId, ride, bytes), bytes);
    return bytes;
}

/**
 * A ride library that keeps real state and can be made to fail.
 *
 * Not a mock of `lib/desktop/library.ts`'s six `invoke()` calls — the tests below never look at
 * what was called. It models the one property the interface promises and the whole feature hangs
 * off: **`import()` resolving is what makes a ride appear in `durableIds()`**. A rejected import
 * leaves nothing behind, exactly as a power cut before fsync does on the real thing.
 */
class RecordingLibrary implements RideLibrary {
    readonly rides = new Map<string, LibraryRide>();
    /** Every library call and every ack, in order — read by the ordering test and nothing else. */
    readonly events: string[] = [];
    /** Keys whose import must fail, as a crash between write and fsync would. */
    failFor = new Set<string>();
    folder = "/library";

    async view(): Promise<LibraryView> {
        return { folder: this.folder, isDefault: true, rides: [...this.rides.values()] };
    }

    async import(ride: RideImport): Promise<{ ride: LibraryRide; imported: boolean }> {
        const key = `${ride.serial}:${ride.epoch}:${ride.objectId}`;
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
            epoch: ride.epoch,
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
            ridePath: `${this.folder}/${ride.objectId}.obcride`,
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

    async durableIds(scope: RideScope): Promise<number[]> {
        this.events.push("durableIds");
        return [...this.rides.values()]
            .filter((r) => r.present && r.serial === scope.serial && r.epoch === scope.epoch)
            .map((r) => r.objectId)
            .sort((a, b) => a - b);
    }

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
}

/** A live client + device pair over the loopback pipe, plus the narrowed source the pull is given. */
function connect(options: { storeEpoch?: number | null } = {}) {
    const link = loopbackLink();
    // `??` would be wrong here: `null` is the case under test (a device with no mounted card),
    // not an absent option.
    const storeEpoch = options.storeEpoch === undefined ? EPOCH_1 : options.storeEpoch;
    const device = new MockDevice(link.device, { storeEpoch });
    void device.run();
    const client = new ProtocolClient(link.host);
    const source: RideSyncSource = rideSyncAccess(client);
    const close = async () => {
        device.stop();
        await client.close();
    };
    return { device, client, source, close };
}

/**
 * The connected device's `(serial, epoch)`, derived exactly as the UI derives it — through
 * `rideScope()` over the two reads every connection already does. Re-read rather than remembered,
 * because that is the point of the epoch: it changes under a running app.
 */
async function scopeNow(client: ProtocolClient): Promise<RideScope> {
    return rideScope(await client.deviceInfo(), await client.identity());
}

/** The `ackRides` writes the device saw. `Command.AckRides` is `2` (§4.4). */
function ackCount(device: MockDevice): number {
    return device.commandLog.filter((cmd) => cmd === Command.AckRides).length;
}

// --- acceptance ----------------------------------------------------------------

describe("pulling rides into the library", () => {
    /** **Acceptance #1.** */
    it("is idempotent: a second pull downloads nothing, duplicates nothing and re-stamps nothing", async () => {
        const { device, client, source, close } = connect();
        const scope = await scopeNow(client);
        seedRide(device, 1, "Dawn Patrol", 1_700_000_000);
        seedRide(device, 2, "Gravel Hour", 1_700_100_000);
        const library = new RecordingLibrary();

        const first = await pullRides(source, library, scope, ctx());
        expect(first.imported).toHaveLength(2);
        expect(first.acked).toEqual([1, 2]);
        expect(first.newlyFlagged).toBe(2);
        const sidecarAfterFirst = device.syncedSidecar();
        const stamps = [...library.rides.values()].map((r) => [r.key, r.importedAt] as const);

        const second = await pullRides(source, library, scope, ctx());
        expect(second.imported).toEqual([]);
        expect(second.alreadyHeld).toBe(2);
        expect(library.rides.size).toBe(2);
        // The ride files were never touched a second time.
        expect(library.events.filter((e) => e.startsWith("imported:"))).toEqual(["imported:1", "imported:2"]);
        expect([...library.rides.values()].map((r) => [r.key, r.importedAt] as const)).toEqual(stamps);

        // The device: the same flags, and — the part that matters for #638 — the same `synced_at`
        // stamps, byte for byte. A re-ack that moved one would push an auto-expiry anchor forward.
        expect(second.acked).toEqual([1, 2]);
        expect(second.newlyFlagged).toBe(0);
        expect(device.syncedSidecar()).toEqual(sidecarAfterFirst);
        await close();
    });

    /** **Acceptance #2**, from the host's side — the Rust side owns the real fsync (see the header). */
    it("leaves a ride un-acked when its durable write fails", async () => {
        const { device, client, source, close } = connect();
        const scope = await scopeNow(client);
        seedRide(device, 1, "Landed", 1_700_000_000);
        seedRide(device, 2, "Interrupted", 1_700_100_000);
        seedRide(device, 3, "Also landed", 1_700_200_000);
        const library = new RecordingLibrary();
        library.failFor.add(rideKey(scope, 2));

        const report = await pullRides(source, library, scope, ctx());

        expect(report.failed.map((f) => f.objectId)).toEqual([2]);
        expect(report.imported.map((r) => r.objectId)).toEqual([1, 3]);
        // The device was told about the two that landed and *not* about the one that did not.
        expect(report.acked).toEqual([1, 3]);
        expect([...device.synced].sort((a, b) => a - b)).toEqual([1, 3]);
        expect(device.synced.has(2)).toBe(false);

        // …and the next pull, with the write working, fetches ride 2 again and only then flags it.
        library.failFor.clear();
        const retry = await pullRides(source, library, scope, ctx());
        expect(retry.imported.map((r) => r.objectId)).toEqual([2]);
        expect([...device.synced].sort((a, b) => a - b)).toEqual([1, 2, 3]);
        await close();
    });

    it("acks strictly after every import has resolved, never as a ride finishes", async () => {
        const { device, client, source, close } = connect();
        const scope = await scopeNow(client);
        seedRide(device, 1, "First", 1_700_000_000);
        seedRide(device, 2, "Second", 1_700_100_000);
        const library = new RecordingLibrary();

        // The device's command log and the library's event log, interleaved by construction: the
        // library pushes an event when it lands a ride, and `ackRides` is the only command this
        // path can send. If the ack were sent per ride, `durableIds` would not be last.
        const ackedAfter: number[] = [];
        const wrapped: RideSyncSource = {
            ...source,
            ackRides: async (ids, signal) => {
                library.events.push("ack");
                ackedAfter.push(library.rides.size);
                return source.ackRides(ids, signal);
            },
        };

        await pullRides(wrapped, library, scope, ctx());

        expect(library.events).toEqual(["imported:1", "imported:2", "durableIds", "ack"]);
        expect(ackedAfter).toEqual([2]);
        expect(ackCount(device)).toBe(1);
        await close();
    });

    /** **Acceptance #3** — the 2026-07-12 incident the iOS `LibraryScopingE2ETests` replays. */
    it("imports both rides when a store-epoch bump recycles an object id", async () => {
        const { device, client, source, close } = connect();
        const oldEra = await scopeNow(client);
        expect(oldEra.epoch).toBe(EPOCH_1);
        seedRide(device, 1, "Old era ride", 1_700_000_000);
        const library = new RecordingLibrary();

        await pullRides(source, library, oldEra, ctx());
        expect(library.rides.size).toBe(1);

        // Chip erase: a fresh epoch, an empty card, ids from 1 again — and a *different* ride
        // lands on the recycled id 1.
        device.reopenIdSpace(EPOCH_2);
        seedRide(device, 1, "New era ride", 1_800_000_000);

        // Re-read, exactly as a reconnect does. The identity read is where the era change becomes
        // visible; nothing else on the wire says it happened.
        const newEra = await scopeNow(client);
        expect(newEra.epoch).toBe(EPOCH_2);
        expect(newEra.serial).toBe(oldEra.serial);

        const after = await pullRides(source, library, newEra, ctx());
        expect(after.imported.map((r) => r.name)).toEqual(["New era ride"]);
        expect(library.rides.size).toBe(2);
        expect([...library.rides.values()].map((r) => r.name).sort()).toEqual([
            "New era ride",
            "Old era ride",
        ]);
        // Two rows, two keys, one object id. A bare-id library would hold one row and would have
        // reported "up to date" instead of fetching the new ride at all.
        expect([...library.rides.keys()].sort()).toEqual(
            [`${SERIAL}:${EPOCH_1}:1`, `${SERIAL}:${EPOCH_2}:1`].sort(),
        );
        expect(new Set([...library.rides.values()].map((r) => r.objectId))).toEqual(new Set([1]));

        // The ack carries the new era's id only; the old era's row is archival and names a ride
        // this device no longer has.
        expect(after.acked).toEqual([1]);
        expect([...device.synced]).toEqual([1]);
        await close();
    });

    /** **Acceptance #4.** */
    it("acks exactly the rides that are durably on disk, not the ones it meant to write", async () => {
        const { device, client, source, close } = connect();
        const scope = await scopeNow(client);
        for (const [id, name] of [
            [1, "One"],
            [2, "Two"],
            [3, "Three"],
        ] as const) {
            seedRide(device, id, name, 1_700_000_000 + id * 1000);
        }
        const library = new RecordingLibrary();
        await pullRides(source, library, scope, ctx());
        expect([...device.synced].sort((a, b) => a - b)).toEqual([1, 2, 3]);
        const report1Stamp = library.rides.get(rideKey(scope, 2))!.importedAt;

        // The rider deletes ride 2's file in the file manager. The record survives; the durable
        // copy does not. Between the two, the filesystem wins: the ride drops out of the ack set
        // and is fetched again — because "do I already have it" is a question about the disk.
        library.loseFile(rideKey(scope, 2));
        expect(await library.durableIds(scope)).toEqual([1, 3]);

        const again = await pullRides(source, library, scope, ctx());
        expect(again.imported).toEqual([]);
        expect(again.repaired.map((r) => r.objectId)).toEqual([2]);
        expect(again.repaired[0].importedAt).toBe(report1Stamp);

        const durable = await library.durableIds(scope);
        expect(again.acked).toEqual(durable);
        expect([...device.synced].sort((a, b) => a - b)).toEqual(durable);
        await close();
    });

    it("re-sends the whole list, which is what heals a device that lost its synced set", async () => {
        const { device, client, source, close } = connect();
        const scope = await scopeNow(client);
        seedRide(device, 1, "Kept", 1_700_000_000);
        const library = new RecordingLibrary();
        await pullRides(source, library, scope, ctx());
        expect([...device.synced]).toEqual([1]);

        // A reflashed card: the sidecar is gone, the ride is not. The library still holds a durable
        // copy, so the next pull re-flags it — the same heal the phone performs on every connect.
        device.reopenIdSpace(EPOCH_1);
        seedRide(device, 1, "Kept", 1_700_000_000);
        expect([...device.synced]).toEqual([]);

        const healed = await pullRides(source, library, scope, ctx());
        expect(healed.imported).toEqual([]);
        expect(healed.acked).toEqual([1]);
        expect([...device.synced]).toEqual([1]);
        await close();
    });

    it("never consults the device's synced flag to decide what to fetch", async () => {
        // Structural, not behavioural: the rideList entry (§7.4, 72 bytes) has no synced field, so
        // the flag is not merely ignored — it is not on the wire. This pins that, because the day
        // it *is* on the wire is the day someone reaches for it as a fetch filter.
        const { device, client, source, close } = connect();
        const scope = await scopeNow(client);
        seedRide(device, 1, "Already synced elsewhere", 1_700_000_000);
        await source.ackRides([1]); // a phone got there first
        expect([...device.synced]).toEqual([1]);

        const library = new RecordingLibrary();
        const report = await pullRides(source, library, scope, ctx());
        expect(report.imported.map((r) => r.objectId)).toEqual([1]);
        expect(Object.keys((await source.listRides()).entries[0])).not.toContain("synced");
        await close();
    });

    it("refuses to import or ack anything from a device with no store epoch", async () => {
        // The short identity read a card-less device serves. Ids from it mean nothing across a
        // replug, so the fail-closed posture is the phone's (#769): no keys, no ack.
        const { device, client, source, close } = connect({ storeEpoch: null });
        const scope = await scopeNow(client);
        seedRide(device, 1, "Unkeyable", 1_700_000_000);
        const library = new RecordingLibrary();

        await expect(pullRides(source, library, scope, ctx())).rejects.toThrow(/store epoch/);
        expect(library.rides.size).toBe(0);
        expect(ackCount(device)).toBe(0);
        expect([...device.synced]).toEqual([]);
        await close();
    });

    it("stores a GPX the device itself would have written, and the object it sent", async () => {
        const { device, client, source, close } = connect();
        const scope = await scopeNow(client);
        const bytes = seedRide(device, 1, "Schauinsland", 1_700_000_000);
        const library = new RecordingLibrary();
        await pullRides(source, library, scope, ctx());

        const key = rideKey(scope, 1);
        // The archive is the device's bytes, unaltered — which is what makes the library lossless
        // while `track_to_gpx` still omits `<time>`.
        expect(library.objects.get(key)).toEqual(bytes);
        const gpx = library.gpx.get(key) ?? "";
        expect(gpx).toContain('<gpx version="1.1" creator="OpenBikeComputer"');
        expect(gpx).toContain("<name>Schauinsland</name>");
        expect(gpx.match(/<trkpt /g) ?? []).toHaveLength(24);
        await close();
    });
});

// --- the preview ----------------------------------------------------------------

describe("the track preview", () => {
    it("is drawn from the ride's own points, keeping the first and the last", () => {
        const ride = rideObject("Long", 1_700_000_000, 5_000);
        const track = previewTrack(ride);
        expect(track.length).toBeLessThanOrEqual(PREVIEW_POINTS);
        expect(track.length).toBeGreaterThan(2);
        expect(track[0]).toEqual([
            Math.round((ride.points[0].lat1e7 / 1e7) * 1e6) / 1e6,
            Math.round((ride.points[0].lon1e7 / 1e7) * 1e6) / 1e6,
        ]);
        const last = ride.points[ride.points.length - 1];
        expect(track[track.length - 1]).toEqual([
            Math.round((last.lat1e7 / 1e7) * 1e6) / 1e6,
            Math.round((last.lon1e7 / 1e7) * 1e6) / 1e6,
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

// --- the narrowed surface --------------------------------------------------------

describe("the device surface the library gets", () => {
    it("carries the ack and nothing else the hosted tier was denied", async () => {
        const { client, source, close } = connect();
        expect(Object.keys(source).sort()).toEqual(["ackRides", "downloadRide", "listRides"]);
        // Frozen, so it cannot be grown in place into a full client the day someone needs a delete.
        expect(Object.isFrozen(source)).toBe(true);
        expect((source as unknown as ProtocolClient).deleteObject).toBeUndefined();
        expect((source as unknown as ProtocolClient).writeConfig).toBeUndefined();
        expect(client.deleteObject).toBeTypeOf("function");
        await close();
    });
});
