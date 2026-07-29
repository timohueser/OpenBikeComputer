/**
 * The desktop host's ride library (E2, #912): `lib/device/library.ts`'s {@link RideLibrary} over six
 * Tauri commands.
 *
 * Deliberately thin. Everything that decides whether this feature is correct — what is fetched,
 * what is deduped, what is acked and *when* — is in `lib/device/library.ts`, over this interface,
 * where it is tested against a fake that is not a mock of these calls. What lives here is the
 * translation between the app's shapes and `serde`'s, and one decision worth stating:
 *
 * **`import()` is awaited for its timing, not just its value.** The Rust side writes the ride
 * object, the GPX and the index, fsyncing each (and the directory entry), and only then resolves.
 * The ack that follows is therefore an ack of bytes that are on the disk. Nothing here may be made
 * "fire and forget" for responsiveness — that would turn a durability predicate into an optimistic
 * one, which is the single way this feature can lose a rider's ride (`obc-ble-interface-spec.md`
 * §4.4).
 */

import { desktop, type RideIndexEntry } from "./invoke";
import type { LibraryRide, LibraryView, RideImport, RideLibrary } from "../device/library";
import type { RideScope } from "../device/rides";

/**
 * `RideIndexEntry` → `LibraryRide`.
 *
 * A rename of two fields and nothing else. The wire carries `rideFile`/`gpxFile` as *basenames*
 * plus the joined absolute paths, because a basename is what the index stores (the GPX folder can
 * move) and a path is what `reveal()` needs — and joining them in JavaScript would have to guess a
 * path separator. Since the GPX-only split the two paths point at different roots: `ridePath` into
 * the internal archive under app data, `gpxPath` into the visible folder.
 */
function toRide(entry: RideIndexEntry): LibraryRide {
    return {
        key: entry.key,
        serial: entry.serial,
        epoch: entry.epoch,
        objectId: entry.objectId,
        name: entry.name,
        startTime: entry.startTime,
        distanceM: entry.distanceM,
        movingTimeS: entry.movingTimeS,
        climbM: entry.climbM,
        points: entry.points,
        bytes: entry.bytes,
        crc32: entry.crc32,
        importedAt: entry.importedAt,
        ridePath: entry.ridePath,
        gpxPath: entry.gpxPath,
        track: entry.track,
        present: entry.present,
        gpxPresent: entry.gpxPresent,
    };
}

export function openRideLibrary(): RideLibrary {
    return {
        async view(): Promise<LibraryView> {
            const view = await desktop.ridesIndex();
            return {
                folder: view.folder,
                isDefault: view.isDefault,
                rides: view.rides.map(toRide),
                migrationWarning: view.migrationWarning ?? null,
            };
        },

        async import(ride: RideImport) {
            const landed = await desktop.ridesImport({
                serial: ride.serial,
                epoch: ride.epoch,
                objectId: ride.objectId,
                name: ride.name,
                startTime: ride.startTime,
                distanceM: ride.distanceM,
                movingTimeS: ride.movingTimeS,
                climbM: ride.climbM,
                points: ride.points,
                crc32: ride.crc32,
                track: ride.track.map((p) => [p[0], p[1]] as [number, number]),
                // The one place this host pays the JSON tax on binary. A ride object is hundreds of
                // kilobytes — two to three orders of magnitude below the map transfers that forced
                // `usb_send_file`'s raw path — and it is written exactly once per ride, behind a
                // transfer that already took longer than the encode will.
                object: Array.from(ride.object),
                gpx: ride.gpx,
            });
            return { ride: toRide(landed.ride), imported: landed.imported };
        },

        durableIds(scope: RideScope): Promise<number[]> {
            // `pullRides` refuses a null epoch before it reaches here; the `?? 0` is only so this
            // signature does not have to lie about the type.
            return desktop.ridesAckSet(scope.serial, scope.epoch ?? 0);
        },

        async readObject(key: string): Promise<Uint8Array> {
            return new Uint8Array(await desktop.ridesRead(key));
        },

        writeGpx: (key: string, gpx: string) => desktop.ridesWriteGpx(key, gpx),
        reveal: (path: string) => desktop.revealFile(path),
        chooseFolder: () => desktop.ridesChooseFolder(),
    };
}
