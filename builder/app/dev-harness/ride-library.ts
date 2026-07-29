/**
 * An in-memory {@link RideLibrary} for the dev harness — so the Rides page (desktop-only in the
 * shipping app: `caps.rideLibrary`, Tauri IPC underneath) can be rendered and driven in a browser.
 *
 * Same hard line as the simulated device (`simulated-device.svelte.ts`): this file lives outside
 * `src/`, no shipping module imports it, and the harness entry point injects it by overriding the
 * platform object (`main.ts`). The model is `library.test.ts`'s `RecordingLibrary`: real state,
 * not a mock — `import()` resolving is what makes a ride appear in `durableIds()`, `readObject`
 * hands back real §7.2 bytes (so the chart-room preview and the GPX auto-repair run the *actual*
 * decode + wasm export), and `writeGpx` flips `gpxPresent` like the real re-export does.
 *
 * The seeds are chosen for the map, not for realism: three rides around Freiburg and the
 * Kaiserstuhl (a cluster, when zoomed out) and one lone ride near Innsbruck (its own badge). One
 * Freiburg ride starts with its GPX "missing", so the panel's quiet auto-repair has something to
 * repair on first open. The simulated *device* (serial 0011223344556677) uses a different serial
 * than these seeds, so "Pull rides from device" lands its three rides as new rows and new tracks.
 */

import {
    previewTrack,
    type LibraryRide,
    type LibraryView,
    type RideImport,
    type RideLibrary,
} from "../src/lib/device/library";
import type { RideScope } from "../src/lib/device/rides";
import { encodeRideObject, type RideObject, type RidePoint } from "../src/lib/usb/objects";

const FOLDER = "/Users/you/Documents/OpenBikeComputer/rides";
const ARCHIVE = "/Users/you/Library/Application Support/obc-desktop/ride-archive";

interface Held {
    ride: LibraryRide;
    object: Uint8Array;
}

/** A loop ride around a center, with a real elevation shape — enough for profile + map + facts. */
function loopRide(
    name: string,
    startTime: number,
    centerLat: number,
    centerLon: number,
    radiusDeg: number,
    climbM: number,
): RideObject {
    const points: RidePoint[] = [];
    const n = 480;
    for (let i = 0; i < n; i++) {
        const a = (i / n) * 2 * Math.PI;
        // A slightly dented loop so no two rides are the same squiggle.
        const wobble = 1 + 0.18 * Math.sin(a * 3 + centerLon);
        points.push({
            tOffsetS: i * 10,
            lat1e7: Math.round((centerLat + radiusDeg * wobble * Math.sin(a)) * 1e6) * 10,
            lon1e7: Math.round((centerLon + radiusDeg * 1.4 * wobble * Math.cos(a)) * 1e6) * 10,
            eleM: 260 + Math.round(climbM * Math.max(0, Math.sin(a * 1.5 + 0.4))),
            hrBpm: 120 + (i % 40),
            cadenceRpm: 78,
            powerW: null,
        });
    }
    const distanceM = Math.round(radiusDeg * 111_000 * 2 * Math.PI * 1.2);
    return {
        version: 2,
        name,
        startTime,
        distanceM,
        movingTimeS: n * 10,
        avgSpeedCms: Math.round((distanceM / (n * 10)) * 100),
        climbM,
        avgHr: 139,
        maxHr: 171,
        avgCadence: 78,
        avgPower: null,
        maxPower: null,
        points,
    };
}

class HarnessRideLibrary implements RideLibrary {
    private readonly held = new Map<string, Held>();
    private nextStamp = 1_753_000_000;

    seed(serial: string, epoch: number, objectId: number, ride: RideObject, gpxPresent: boolean): void {
        const key = `${serial}:${epoch}:${objectId}`;
        const object = encodeRideObject(ride);
        this.held.set(key, {
            object,
            ride: {
                key,
                serial,
                epoch,
                objectId,
                name: ride.name,
                startTime: ride.startTime,
                distanceM: ride.distanceM,
                movingTimeS: ride.movingTimeS,
                climbM: ride.climbM,
                points: ride.points.length,
                bytes: object.length,
                crc32: 0,
                importedAt: this.nextStamp++,
                ridePath: `${ARCHIVE}/${objectId}.obcride`,
                gpxPath: `${FOLDER}/${ride.name}.gpx`,
                track: previewTrack(ride),
                present: true,
                gpxPresent,
            },
        });
    }

    async view(): Promise<LibraryView> {
        return { folder: FOLDER, isDefault: true, rides: [...this.held.values()].map((h) => h.ride) };
    }

    async import(ride: RideImport): Promise<{ ride: LibraryRide; imported: boolean }> {
        const key = `${ride.serial}:${ride.epoch}:${ride.objectId}`;
        const existing = this.held.get(key);
        if (existing) {
            // The repair path, mirrored from the real thing: same names, same `importedAt`.
            const repaired = { ...existing.ride, present: true, gpxPresent: true };
            this.held.set(key, { ride: repaired, object: ride.object });
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
            importedAt: this.nextStamp++,
            ridePath: `${ARCHIVE}/${ride.objectId}.obcride`,
            gpxPath: `${FOLDER}/${ride.name || `ride-${ride.objectId}`}.gpx`,
            track: ride.track.map((p) => [p[0], p[1]] as [number, number]),
            present: true,
            gpxPresent: true,
        };
        this.held.set(key, { ride: landed, object: ride.object });
        return { ride: landed, imported: true };
    }

    async durableIds(scope: RideScope): Promise<number[]> {
        return [...this.held.values()]
            .filter((h) => h.ride.present && h.ride.serial === scope.serial && h.ride.epoch === scope.epoch)
            .map((h) => h.ride.objectId)
            .sort((a, b) => a - b);
    }

    async readObject(key: string): Promise<Uint8Array> {
        const held = this.held.get(key);
        if (!held) throw new Error(`no ride ${key} in this library`);
        return held.object;
    }

    async writeGpx(key: string, gpx: string): Promise<string> {
        const held = this.held.get(key);
        if (!held) throw new Error(`no ride ${key} in this library`);
        void gpx;
        this.held.set(key, { ...held, ride: { ...held.ride, gpxPresent: true } });
        return held.ride.gpxPath;
    }

    async reveal(path: string): Promise<void> {
        console.info(`dev-harness: would reveal ${path} in the file manager`);
    }

    async chooseFolder(): Promise<string | null> {
        console.info("dev-harness: would open the OS folder chooser");
        return null;
    }
}

/** The one library instance the harness serves — Device page and Rides page share it, like the
 *  real app's two callers share the one folder. */
let singleton: HarnessRideLibrary | null = null;

export function harnessRideLibrary(): RideLibrary {
    if (singleton) return singleton;
    const lib = new HarnessRideLibrary();
    const serial = "OBC-24-000111"; // an older device's pulls — NOT the simulated device's serial
    const epoch = 7;
    // The Freiburg / Kaiserstuhl cluster…
    lib.seed(serial, epoch, 1, loopRide("Schauinsland classic", 1_784_608_800, 47.91, 7.9, 0.045, 1120), true);
    lib.seed(serial, epoch, 2, loopRide("Rosskopf after work", 1_784_090_400, 48.01, 7.9, 0.028, 510), true);
    // …one of which lost its GPX, so the auto-repair has work on first open…
    lib.seed(serial, epoch, 3, loopRide("Kaiserstuhl gravel", 1_783_917_600, 48.09, 7.66, 0.038, 420), false);
    // …and a lone ride far enough away for its own cluster badge.
    lib.seed(serial, epoch, 4, loopRide("Inntal shakedown", 1_782_712_800, 47.26, 11.39, 0.05, 640), true);
    singleton = lib;
    return lib;
}
