import { describe, expect, it, vi } from "vitest";
import { openRideLibrary } from "./library";
import { desktop, type RideIndexEntry } from "./invoke";

vi.mock("./invoke", () => ({
    desktop: { ridesIndex: vi.fn(), ridesImport: vi.fn() },
}));

describe("desktop ride identity at the JSON boundary", () => {
    it("uses decimal strings in both directions and bigint in the library", async () => {
        const library = openRideLibrary();
        for (const storeId of ["a1b2c3d4000000000000000000000000", "a1b2c3d4000000000000000000000001"]) {
            for (const objectId of [65536n, 9007199254740993n, 18446744073709551615n]) {
                const key = `OBC-24-000317:${storeId}:${objectId}`;
                const entry: RideIndexEntry = {
                    key, serial: "OBC-24-000317", storeId, objectId: objectId.toString(),
                    name: "Ride", startTime: 0, distanceM: 0, movingTimeS: 0, climbM: 0,
                    points: 1, bytes: 3, crc32: 0, importedAt: 1,
                    rideFile: "ride.obcride", gpxFile: "ride.gpx",
                    ridePath: "/archive/ride.obcride", gpxPath: "/rides/ride.gpx",
                    track: [], present: true, gpxPresent: true,
                };
                vi.mocked(desktop.ridesImport).mockImplementation(async (request) => {
                    const json = JSON.parse(JSON.stringify(request));
                    expect(json.storeId).toBe(storeId);
                    expect(json.objectId).toBe(objectId.toString());
                    return JSON.parse(JSON.stringify({ ride: entry, imported: true }));
                });
                const imported = await library.import({
                    ...entry, objectId, object: new Uint8Array([1, 2, 3]), gpx: "<gpx/>",
                });
                expect(imported.ride.key).toBe(key);
                expect(imported.ride.storeId).toBe(storeId);
                expect(imported.ride.objectId).toBe(objectId);

                vi.mocked(desktop.ridesIndex).mockResolvedValue(
                    JSON.parse(JSON.stringify({ folder: "/rides", isDefault: true, rides: [entry] })),
                );
                expect((await library.view()).rides[0]).toEqual(imported.ride);
            }
        }
    });
});
