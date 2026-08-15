/**
 * The drift guard between this client and the wire (C3, issue #902).
 *
 * `specs/vectors/` is the contract three implementations agree on: the firmware pins it with
 * `cargo test -p obc-vectors`, the iOS app with `swift test`, the browser's conversion bridge with
 * `convert/bridge.test.ts` — and now this client, which is the fourth. A file in that directory is
 * not a fixture in the "some bytes I captured" sense; it is the spec made executable. A divergence
 * here is a bug here.
 *
 * Two kinds of assertion, and both are needed:
 *
 * 1. **Codec identity** — every control message and object layout encodes to, and decodes from, the
 *    checked-in bytes. That catches a field at the wrong offset.
 * 2. **Round-trip over the pipe** — the same bytes pushed through the loopback transport, uploaded
 *    and downloaded as real objects with a real CRC. That catches everything a codec test cannot:
 *    a transfer that assumes a read returns a whole object, a CRC folded over the wrong slice, a
 *    result correlated against the wrong id.
 */

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { Crc32 } from "./crc32";
import { REFERENCE_OBCM_VERSION, loopbackDevice, type MockDevice } from "./loopback";
import {
    decodeRideList,
    decodeRideObject,
    decodeRouteList,
    decodeTripList,
    decodeTripObject,
    encodeRideObject,
    encodeTripObject,
    type RouteListEntry,
    type TripListEntry,
} from "./objects";
import {
    NEW_OBJECT_ID,
    ObjectType,
    Op,
    SINGLETON_OBJECT_ID,
    TransferStatus,
    decodeConfig,
    decodeStatusMessage,
    decodeTransferControl,
    decodeVersionRead,
    FEATURE_WEATHER,
    knownWeatherRefresh,
    WeatherRefresh,
    WEATHER_REFRESH_DEFAULT,
    WEATHER_REFRESH_MINUTES,
    encodeAckRides,
    encodeConfig,
    encodeSetClock,
    encodeSetRouteRetention,
    encodeStatusMessage,
    encodeTransferControl,
    encodeVersionRead,
    setPartId,
    viewOf,
} from "./protocol";

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
const MANIFEST = JSON.parse(readFileSync(join(ROOT, "specs/vectors", "manifest.json"), "utf8")) as {
    crc32_check: { input: string; value: string };
    protocol_version: number;
    fixtures: Record<string, Record<string, unknown>>;
};

/** Fail on the first differing byte with its index, instead of dumping two long arrays. */
function expectSameBytes(actual: Uint8Array, expected: Uint8Array, what: string): void {
    const n = Math.min(actual.length, expected.length);
    for (let i = 0; i < n; i++) {
        if (actual[i] !== expected[i]) {
            throw new Error(
                `${what}: first difference at byte ${i} — this client produced 0x${actual[i].toString(16)}, ` +
                    `the fixture has 0x${expected[i].toString(16)} (lengths ${actual.length} vs ${expected.length})`,
            );
        }
    }
    expect(actual.length, `${what}: length`).toBe(expected.length);
}

/** The manifest's hex strings, as the numbers the codecs deal in. */
const hex = (s: string): number => Number.parseInt(s, 16) >>> 0;

/** A ride on the device with nothing in it — enough for an ack to have something to flag. */
function seedEmptyRide(device: MockDevice, objectId: number): void {
    device.seedRide(
        {
            objectId,
            byteLen: 0,
            startTime: 0,
            distanceM: 0,
            movingTimeS: 0,
            avgSpeedCms: 0,
            climbM: 0,
            name: `ride ${objectId}`,
        },
        new Uint8Array(0),
    );
}

describe("CRC-32 against the fixtures", () => {
    it("reproduces the manifest's check value", () => {
        expect(Crc32.of(new TextEncoder().encode(MANIFEST.crc32_check.input))).toBe(hex(MANIFEST.crc32_check.value));
    });

    /**
     * The check value alone only proves the polynomial. These are real objects fingerprinted by the
     * Rust implementation, so they prove this hasher agrees with the one the device runs — which is
     * what every commit in the protocol turns on.
     */
    it.each([
        ["route-waypoints.obcr", "0x1BFB6E3C"],
        ["route-plain.obcr", "0x1557AE0B"],
        ["trip-v1.bin", "0xA3C5D591"],
    ])("agrees with the device's fingerprint of %s", (file, expected) => {
        expect(Crc32.of(vector(file))).toBe(hex(expected));
    });

    it("is unaffected by how the bytes are sliced", () => {
        // A bulk endpoint delivers arbitrary segmentation, so a CRC that only worked on whole
        // buffers would pass every unit test and fail on hardware.
        const data = vector("route-waypoints.obcr");
        const whole = Crc32.of(data);
        for (const split of [0, 1, 7, 64, 307, data.length]) {
            const h = new Crc32();
            h.update(data.subarray(0, split));
            h.update(data.subarray(split));
            expect(h.value(), `split at ${split}`).toBe(whole);
        }
    });
});

describe("control-plane codecs", () => {
    it("round-trips the transfer descriptors", () => {
        const upload = decodeTransferControl(vector("transfer-upload-start.bin"));
        expect(upload).toEqual({
            op: Op.Upload,
            type: ObjectType.Route,
            objectId: NEW_OBJECT_ID,
            totalLen: 308,
            crc32: hex("0x1BFB6E3C"),
        });
        expectSameBytes(encodeTransferControl(upload), vector("transfer-upload-start.bin"), "upload descriptor");

        const download = decodeTransferControl(vector("transfer-download-request.bin"));
        expect(download).toEqual({
            op: Op.Download,
            type: ObjectType.RideList,
            objectId: SINGLETON_OBJECT_ID,
            totalLen: 0,
            crc32: 0,
        });
        expectSameBytes(encodeTransferControl(download), vector("transfer-download-request.bin"), "download request");

        const abort = decodeTransferControl(vector("transfer-abort.bin"));
        expect(abort.op).toBe(Op.Abort);
        expectSameBytes(encodeTransferControl(abort), vector("transfer-abort.bin"), "abort descriptor");

        const shard = decodeTransferControl(vector("transfer-set-shard.bin"));
        expect(shard).toMatchObject({ op: Op.Upload, type: ObjectType.MapShard, objectId: 0x0802 });
        expect(setPartId(8, 2)).toBe(shard.objectId);
        expectSameBytes(encodeTransferControl(shard), vector("transfer-set-shard.bin"), "set shard descriptor");

        const manifest = decodeTransferControl(vector("transfer-set-manifest.bin"));
        expect(manifest).toMatchObject({ op: Op.Upload, type: ObjectType.MapSet, objectId: NEW_OBJECT_ID });
        expectSameBytes(
            encodeTransferControl(manifest),
            vector("transfer-set-manifest.bin"),
            "set manifest descriptor",
        );
    });

    it("round-trips every status message", () => {
        const result = decodeStatusMessage(vector("status-transfer-result.bin"));
        expect(result).toEqual({
            msg: "transferResult",
            objectId: 7,
            status: TransferStatus.Committed,
            committedOffset: 308,
        });

        const full = decodeStatusMessage(vector("status-transfer-storage-full.bin"));
        expect(full).toEqual({
            msg: "transferResult",
            objectId: NEW_OBJECT_ID,
            status: TransferStatus.StorageFull,
            committedOffset: 0,
        });

        const changed = decodeStatusMessage(vector("status-store-changed.bin"));
        expect(changed).toEqual({ msg: "storeChanged", type: ObjectType.Route, revision: 42 });

        const ack = decodeStatusMessage(vector("status-command-result-ack.bin"));
        expect(ack).toEqual({ msg: "commandResult", command: 2, status: 0, detail: 3 });

        const announce = decodeStatusMessage(vector("status-download-announce.bin"));
        expect(announce).toEqual({
            msg: "downloadAnnounce",
            descriptor: {
                op: Op.Download,
                type: ObjectType.Route,
                objectId: 7,
                totalLen: 308,
                crc32: hex("0x1BFB6E3C"),
            },
        });

        for (const file of [
            "status-transfer-result.bin",
            "status-transfer-storage-full.bin",
            "status-store-changed.bin",
            "status-command-result-ack.bin",
            "status-download-announce.bin",
        ]) {
            const decoded = decodeStatusMessage(vector(file));
            expect(decoded, file).not.toBeNull();
            expectSameBytes(encodeStatusMessage(decoded!), vector(file), file);
        }
    });

    it("ignores an unknown status discriminator instead of failing", () => {
        // The forward-compatibility hinge: a firmware that adds a message type must not break a
        // browser tab that predates it.
        expect(decodeStatusMessage(new Uint8Array([99, 1, 2, 3]))).toBeNull();
    });

    it("encodes the command writes", () => {
        expectSameBytes(encodeAckRides([3, 5, 9]), vector("command-ack-rides.bin"), "ackRides");
        expectSameBytes(encodeSetClock(1783598400, 120), vector("command-set-clock.bin"), "setClock");
        expectSameBytes(encodeSetRouteRetention(7, 3), vector("command-set-route-retention.bin"), "setRouteRetention");
    });

    it("refuses the setClock values the device would answer `error` to", () => {
        // Keeping the range checks host-side means a bogus clock fails in the tab rather than
        // costing a round trip — and it means both ends share one definition of "valid".
        expect(() => encodeSetClock(1_577_836_799, 0)).toThrow(RangeError);
        expect(() => encodeSetClock(1783598400, 841)).toThrow(RangeError);
        expect(() => encodeSetClock(1783598400, -841)).toThrow(RangeError);
        expect(() => encodeSetRouteRetention(7, 6)).toThrow(RangeError);
    });

    it("round-trips the identity read at all four lengths", () => {
        // 11 bytes: the full read a current firmware serves. `featureBits` is the capability word
        // (§1, WX3) — bit 0 is the Weather Request contract, which is the phone's path and nothing
        // this host acts on; it is mirrored so the read decodes whole rather than being silently
        // truncated by the one consumer that does not care about it.
        const features = decodeVersionRead(vector("version-read-features.bin"));
        expect(features.featureBits).toBe(FEATURE_WEATHER);
        expectSameBytes(encodeVersionRead(features), vector("version-read-features.bin"), "version-read-features");

        // 7 bytes: a firmware predating the capability word. Absent, not zero — both mean "no
        // optional contracts", but only one of them is something the device actually said.
        // `obcmVersion` is the OBCM *map-format* version the device's
        // reader reads — the fact `OBCC_Spec.md` §10 filters the catalog on, and a different
        // number in a different sequence from the protocol `version` beside it.
        const full = decodeVersionRead(vector("version-read.bin"));
        expect(full).toEqual({
            version: 2,
            storeEpoch: hex("0xA1B2C3D4"),
            obcmVersion: REFERENCE_OBCM_VERSION,
            featureBits: null,
        });
        expectSameBytes(encodeVersionRead(full), vector("version-read.bin"), "version-read");

        // 6 bytes: a firmware predating the field. Unknown, not zero — `obcmVersion: 0` would read
        // as "supports OBCM v0" and refuse every real map, where null takes §6(c)'s
        // no-known-target-firmware branch and offers the download stating the version.
        const noObcm = decodeVersionRead(vector("version-read-noobcm.bin"));
        expect(noObcm).toEqual({
            version: 2,
            storeEpoch: hex("0xA1B2C3D4"),
            obcmVersion: null,
            featureBits: null,
        });
        expectSameBytes(encodeVersionRead(noObcm), vector("version-read-noobcm.bin"), "version-read-noobcm");

        // 2 bytes: no card means no era to name, so the device serves the version alone. That
        // absent epoch has to decode as "none", never as epoch 0 — 0 is a legal era, and
        // conflating them would let a peer stamp id-keyed state under an era it never read. There
        // is no room for an obcm byte after an epoch that is not there, either.
        const short = decodeVersionRead(vector("version-read-nostore.bin"));
        expect(short).toEqual({ version: 2, storeEpoch: null, obcmVersion: null, featureBits: null });
        expectSameBytes(encodeVersionRead(short), vector("version-read-nostore.bin"), "version-read-nostore");
    });

    it("ignores identity bytes past the fields it knows", () => {
        // The append-only rule the `obcm_version` byte itself rode in on (§1): a longer read from a
        // newer firmware decodes to what this build understands rather than failing, which is why
        // appending the field needed no `PROTOCOL_VERSION` bump.
        const future = new Uint8Array([...vector("version-read-features.bin"), 0xee, 0xee]);
        expect(decodeVersionRead(future)).toEqual({
            version: 2,
            storeEpoch: decodeVersionRead(vector("version-read-features.bin")).storeEpoch,
            obcmVersion: decodeVersionRead(vector("version-read-features.bin")).obcmVersion,
            featureBits: FEATURE_WEATHER,
        });

        // A *partial* capability word is absent, not the bytes that turned up: three bytes of a
        // u32 are a broken read, not a small feature set, and decoding them could claim a contract
        // the device never announced.
        for (const len of [8, 9, 10]) {
            expect(decodeVersionRead(vector("version-read-features.bin").subarray(0, len)).featureBits).toBeNull();
        }
    });

    it("round-trips the Config object", () => {
        const config = decodeConfig(vector("config-v1.bin"));
        // An absent `weatherRefresh` means *device default*, not `Off` — collapsing the two would
        // silently disable weather every time this host round-tripped a Config to rename a device.
        expect(config).toEqual({ name: "OBC Tourer", units: 0, weatherRefresh: null });
        expectSameBytes(encodeConfig(config), vector("config-v1.bin"), "config-v1");

        const withRefresh = decodeConfig(vector("config-weather-refresh.bin"));
        expect(withRefresh.weatherRefresh).toBe(WeatherRefresh.every60);
        expect(knownWeatherRefresh(withRefresh.weatherRefresh)).toBe(WeatherRefresh.every60);
        expect(WEATHER_REFRESH_MINUTES[WeatherRefresh.every60]).toBe(60);
        expectSameBytes(encodeConfig(withRefresh), vector("config-weather-refresh.bin"), "config-weather-refresh");
    });

    // §11.8's read direction, which this host is always on. The rule is asymmetric on purpose: a
    // phone → device *write* must reject an interval the device cannot honour, but a *reader* must
    // not, because an unrecognised value arriving from a device is a newer device rather than a
    // broken one. Were this strict, appending a fifth interval — an ordinary append to an
    // append-only enum — would break every shipped reader, and a host would no longer be able to
    // read Config even to rename the device.
    it("tolerates a refresh interval it does not know, and never calls it Off", () => {
        const bytes = vector("config-weather-refresh-unknown.bin");
        const config = decodeConfig(bytes); // must not throw
        expect(config.name).toBe("OBC Horizon");

        expect(config.weatherRefresh).toBe(200);
        expect(knownWeatherRefresh(config.weatherRefresh)).toBeNull();
        // Unknown is its own state: not Off, and not the default. Collapsing it to either would
        // misreport the rider's own setting back to them.
        expect(knownWeatherRefresh(config.weatherRefresh)).not.toBe(WeatherRefresh.off);
        expect(knownWeatherRefresh(config.weatherRefresh)).not.toBe(WEATHER_REFRESH_DEFAULT);
        // ...and it is distinguishable from *absent*, which the raw field alone can say.
        expect(config.weatherRefresh).not.toBeNull();
        expect(decodeConfig(vector("config-v1.bin")).weatherRefresh).toBeNull();

        // The byte survives verbatim, so a host cannot launder a value it did not understand.
        expectSameBytes(encodeConfig(config), bytes, "config-weather-refresh-unknown");
    });

    it("maps every known refresh discriminant, and nothing else", () => {
        for (const value of Object.values(WeatherRefresh)) {
            expect(knownWeatherRefresh(value)).toBe(value);
        }
        for (const unknown of [5, 9, 200, 255]) {
            expect(knownWeatherRefresh(unknown)).toBeNull();
        }
        expect(knownWeatherRefresh(null)).toBeNull();
        expect(WEATHER_REFRESH_MINUTES[WeatherRefresh.off]).toBeNull(); // Off has no interval
    });
});

describe("object codecs", () => {
    it("decodes the routeList, expiry tail and all", () => {
        const { header, entries } = decodeRouteList(vector("route-list.bin"));
        expect(header).toEqual({ entryLen: 84, count: 3, total: 3 });
        expect(entries.map((e) => e.objectId)).toEqual([7, 8, 9]);
        expect(entries[0]).toEqual({
            objectId: 7,
            byteLen: 308,
            distanceM: 2207,
            ascentM: 76,
            pointCount: 9,
            waypointCount: 2,
            name: "Vector Loop",
            crc32: hex("0x1BFB6E3C"),
            expiresAt: 1784808000,
            retention: 3,
        });
        // Two of the three retention states the fixture covers: a live countdown, a clock that has
        // not started, and `Never`.
        expect(entries[1].expiresAt).toBe(0);
        expect(entries[1].retention).toBe(1);
        expect(entries[2].retention).toBe(0);
    });

    it("decodes the tripList", () => {
        const { header, entries } = decodeTripList(vector("trip-list.bin"));
        expect(header.entryLen).toBe(76);
        expect(entries).toEqual([
            {
                objectId: 1,
                byteLen: 62,
                totalDistanceM: 4414,
                totalAscentM: 152,
                stageCount: 3,
                name: "Alpen Traverse",
                crc32: hex("0xA3C5D591"),
            },
        ]);
    });

    it("round-trips the ride object at both versions", () => {
        const v1 = decodeRideObject(vector("ride-v1.bin"));
        expect(v1.version).toBe(1);
        expect(v1.name).toBe("Höhenweg");
        expect(v1.startTime).toBe(1751450000);
        expect(v1.distanceM).toBe(42500);
        expect(v1.points).toHaveLength(3);
        expect(v1.points[2].eleM).toBeNull();
        // A v1 ride has no sensor fields at all — they must read absent, not zero.
        expect(v1.avgHr).toBeNull();
        expect(v1.points[0].hrBpm).toBeNull();
        expectSameBytes(encodeRideObject(v1), vector("ride-v1.bin"), "ride-v1");

        const v2 = decodeRideObject(vector("ride-v2.bin"));
        expect(v2.version).toBe(2);
        expect(v2.name).toBe("Sensor Ride");
        expect({ avgHr: v2.avgHr, maxHr: v2.maxHr, avgCadence: v2.avgCadence, avgPower: v2.avgPower, maxPower: v2.maxPower })
            .toEqual({ avgHr: 142, maxHr: 176, avgCadence: 85, avgPower: 210, maxPower: 480 });
        expect(v2.points[0]).toEqual({
            tOffsetS: 0,
            lat1e7: 480000000,
            lon1e7: 78000000,
            eleM: 214,
            hrBpm: 140,
            cadenceRpm: 84,
            powerW: 205,
        });
        // The fixture mixes present and absent samples on purpose; the sentinels must not survive
        // into decoded values.
        expect(v2.points[1]).toMatchObject({ hrBpm: null, cadenceRpm: null, powerW: null });
        expect(v2.points[2]).toMatchObject({ eleM: null, hrBpm: 150, cadenceRpm: null, powerW: 215 });
        expectSameBytes(encodeRideObject(v2), vector("ride-v2.bin"), "ride-v2");
    });

    it("round-trips the trip object, dangling stage included", () => {
        const trip = decodeTripObject(vector("trip-v1.bin"));
        // 99 references a route that does not exist. The device serves the trip verbatim and never
        // rewrites it, so a decoder that silently dropped the id would make the peer re-upload a
        // trip that had quietly lost a stage.
        expect(trip).toEqual({ name: "Alpen Traverse", stages: [7, 8, 99] });
        expectSameBytes(encodeTripObject(trip), vector("trip-v1.bin"), "trip-v1");
    });

    it("rejects a ride object whose length disagrees with its header", () => {
        const truncated = vector("ride-v1.bin").subarray(0, 60);
        expect(() => decodeRideObject(truncated)).toThrow(/should be 74 bytes/);
    });
});

describe("round-trip over the loopback pipe", () => {
    /** The routeList fixture's three entries, as the device would hold them. */
    const ROUTE_ENTRIES: RouteListEntry[] = [
        {
            objectId: 7,
            byteLen: 308,
            distanceM: 2207,
            ascentM: 76,
            pointCount: 9,
            waypointCount: 2,
            name: "Vector Loop",
            crc32: hex("0x1BFB6E3C"),
            expiresAt: 1784808000,
            retention: 3,
        },
        {
            objectId: 8,
            byteLen: 220,
            distanceM: 2207,
            ascentM: 76,
            pointCount: 9,
            waypointCount: 0,
            name: "Vector Loop",
            crc32: hex("0x1557AE0B"),
            expiresAt: 0,
            retention: 1,
        },
        {
            objectId: 9,
            byteLen: 220,
            distanceM: 2207,
            ascentM: 76,
            pointCount: 9,
            waypointCount: 0,
            name: "Vector Loop",
            crc32: hex("0x1557AE0B"),
            expiresAt: 0,
            retention: 0,
        },
    ];

    const TRIP_ENTRY: TripListEntry = {
        objectId: 1,
        byteLen: 62,
        totalDistanceM: 4414,
        totalAscentM: 152,
        stageCount: 3,
        name: "Alpen Traverse",
        crc32: hex("0xA3C5D591"),
    };

    it.each([
        ["route-waypoints.obcr", ObjectType.Route],
        ["route-plain.obcr", ObjectType.Route],
        ["update-container-v1.bin", ObjectType.FwImage],
    ])("uploads %s and reads back the same bytes", async (file, type) => {
        const bytes = vector(file);
        const { client, device, close } = loopbackDevice();
        try {
            // A firmware image is a singleton stage (id 0, no id assigned); everything else uploads
            // fresh and gets an id back.
            const target = type === ObjectType.FwImage ? SINGLETON_OBJECT_ID : NEW_OBJECT_ID;
            const { objectId, committedOffset } = await client.upload(type, target, bytes);
            expect(committedOffset).toBe(bytes.length);
            if (type === ObjectType.FwImage) {
                expect(objectId).toBe(SINGLETON_OBJECT_ID);
                expectSameBytes(device.stagedFirmware!, bytes, `${file} staged`);
                return;
            }
            expectSameBytes(device.stored(type, objectId)!, bytes, `${file} stored`);
            expectSameBytes(await client.download(type, objectId), bytes, `${file} downloaded`);
        } finally {
            await close();
        }
    });

    it.each([
        ["ride-v1.bin", ObjectType.Ride],
        ["ride-v2.bin", ObjectType.Ride],
        ["trip-v1.bin", ObjectType.Trip],
    ])("downloads %s byte-for-byte", async (file, type) => {
        const bytes = vector(file);
        const { client, device, close } = loopbackDevice();
        try {
            if (type === ObjectType.Ride) {
                device.seedRide(
                    {
                        objectId: 3,
                        byteLen: bytes.length,
                        startTime: 0,
                        distanceM: 0,
                        movingTimeS: 0,
                        avgSpeedCms: 0,
                        climbM: 0,
                        name: "seeded",
                    },
                    bytes,
                );
            } else {
                device.seedTrip({ ...TRIP_ENTRY, byteLen: bytes.length }, bytes);
            }
            const id = type === ObjectType.Ride ? 3 : TRIP_ENTRY.objectId;
            expectSameBytes(await client.download(type, id), bytes, file);
        } finally {
            await close();
        }
    });

    it("serves the routeList and tripList fixtures over the wire", async () => {
        const { client, device, close } = loopbackDevice();
        try {
            for (const entry of ROUTE_ENTRIES) device.seedRoute(entry);
            device.seedTrip(TRIP_ENTRY, vector("trip-v1.bin"));

            // The bytes first — this is the fixture crossing a real transfer, announce and CRC
            // included, not a codec called in isolation.
            expectSameBytes(
                await client.download(ObjectType.RouteList, SINGLETON_OBJECT_ID),
                vector("route-list.bin"),
                "route-list.bin",
            );
            expectSameBytes(
                await client.download(ObjectType.TripList, SINGLETON_OBJECT_ID),
                vector("trip-list.bin"),
                "trip-list.bin",
            );

            // …then the decoded view the UI actually consumes.
            const routes = await client.listRoutes();
            expect(routes.truncated).toBe(false);
            expect(routes.entries).toEqual(ROUTE_ENTRIES);
            const trips = await client.listTrips();
            expect(trips.entries).toEqual([TRIP_ENTRY]);
        } finally {
            await close();
        }
    });

    // The device role, which is §11.8's *strict* half — the mirror image of the tolerant read test
    // above. A simulated device that quietly stored an interval it does not know would model a
    // firmware that lies to the rider about their own setting.
    it("refuses a Config write naming a refresh interval it cannot honour", async () => {
        const { client, close } = loopbackDevice();
        try {
            const before = await client.readConfig();
            await client.writeConfig({ name: "Rejected", units: 1, weatherRefresh: 200 });
            expect(await client.readConfig(), "a refused write changes nothing at all").toEqual(before);

            // A known interval is accepted, name and all.
            await client.writeConfig({ name: "Accepted", units: 1, weatherRefresh: WeatherRefresh.every120 });
            const stored = await client.readConfig();
            expect(stored.name).toBe("Accepted");
            expect(stored.weatherRefresh).toBe(WeatherRefresh.every120);

            // ...and an *absent* field is not a request to reset it (§7.3): the rename lands, the
            // interval the rider chose survives. This is the case an old app's rename hits.
            await client.writeConfig({ name: "Renamed", units: 1, weatherRefresh: null });
            const afterRename = await client.readConfig();
            expect(afterRename.name).toBe("Renamed");
            expect(afterRename.weatherRefresh).toBe(WeatherRefresh.every120);
        } finally {
            await close();
        }
    });

    it("serves an empty rideList", async () => {
        const { client, close } = loopbackDevice();
        try {
            const rides = await client.listRides();
            expect(rides.entries).toEqual([]);
            expect(rides.truncated).toBe(false);
        } finally {
            await close();
        }
    });

    it("reads the identity and device info a connection starts with", async () => {
        const { client, close } = loopbackDevice();
        try {
            expect(await client.identity()).toEqual({
                version: MANIFEST.protocol_version,
                storeEpoch: hex("0xA1B2C3D4"),
                obcmVersion: REFERENCE_OBCM_VERSION,
                featureBits: 0,
            });
            expect((await client.deviceInfo()).firmwareRevision).toBe("0.4.0+abc1234");
        } finally {
            await close();
        }
    });

    /**
     * `ackRides` over USB (E1, #911).
     *
     * There is deliberately **no `command-ack-rides-usb.bin`**. A USB ack is the checked-in
     * `command-ack-rides.bin` bytes carried on control selector 1, and a second file with identical
     * contents would assert the opposite of what is true — that the transports differ. So the
     * fixture is reused and the *routing* is what gets pinned: the same bytes, over the USB
     * envelope, reach the same handler and move the same sidecar. (The firmware side is structural
     * rather than tested here: `usb::control`'s `COMMAND` arm and `ble::control`'s `command` write
     * both call one `link::command::run_command`.)
     */
    it("acks rides over USB with the same bytes the BLE command carries", async () => {
        const { client, device, close } = loopbackDevice();
        try {
            for (const id of [3, 5, 9]) seedEmptyRide(device, id);
            // The fixture's own ids: `count` 3 · 3, 5, 9.
            expectSameBytes(encodeAckRides([3, 5, 9]), vector("command-ack-rides.bin"), "ackRides");
            expect(await client.ackRides([3, 5, 9])).toBe(3);
            expect([...device.synced].sort((a, b) => a - b)).toEqual([3, 5, 9]);
        } finally {
            await close();
        }
    });

    /**
     * Both merge orders, over the wire, read back off the persisted sidecar.
     *
     * The claim E1 rests on is that a desktop ack and a phone heal need no coordination — no
     * per-sink field, no ownership, no new command — because the ack is add-only and idempotent.
     * The sharp end is the middle case: a phone acking its own library must not un-flag a ride the
     * desktop already fsynced and flagged, and "the phone didn't list it" is not evidence the ride
     * is unsynced. Read off `syncedSidecar()` — the `/tracks/SYNCED.SET` bytes the firmware writes
     * — rather than off a flag, so the thing that actually persists is the thing checked.
     *
     * **What commutes is the set and the stamps, not the file.** The sidecar serialises entries in
     * *insertion* order, so acking 3,5 then 5,9 lays them out `3,5,9` and the reverse lays them out
     * `5,9,3`: same synced rides, same stamps, different bytes. Worth knowing before writing a test
     * that compares two devices' sidecars byte-for-byte — #904's pin compares one device's sidecar
     * across a session, which is a different question and stays valid.
     */
    it("merges a desktop ack and a phone heal to the same synced set in either order", async () => {
        const DESKTOP = [3, 5]; // fsynced into the desktop library
        const PHONE = [5, 9]; // the phone's library — it never held ride 3

        /** The sidecar decoded to `id → synced_at`, which is what the file *means*. */
        function entriesOf(sidecar: Uint8Array): Map<number, number> {
            const view = viewOf(sidecar);
            const count = view.getUint16(6, true);
            const out = new Map<number, number>();
            for (let i = 0; i < count; i++) {
                const at = 8 + i * 6;
                out.set(view.getUint16(at, true), view.getUint32(at + 2, true));
            }
            return out;
        }

        async function ackedInOrder(first: number[], second: number[]): Promise<Map<number, number>> {
            const { client, device, close } = loopbackDevice();
            try {
                for (const id of [3, 5, 9]) seedEmptyRide(device, id);
                // The counts are the other half of the claim: the second ack only ever *adds*, so
                // whichever sink goes second reports exactly the ids the first one didn't hold.
                expect(await client.ackRides(first)).toBe(2);
                expect(await client.ackRides(second)).toBe(1);
                return entriesOf(device.syncedSidecar());
            } finally {
                await close();
            }
        }

        const desktopFirst = await ackedInOrder(DESKTOP, PHONE);
        const phoneFirst = await ackedInOrder(PHONE, DESKTOP);

        // All three flagged either way — in particular ride 3, which the phone's ack omitted and
        // which a "the peer's list is the truth" reading would have cleared.
        expect([...desktopFirst.keys()].sort((a, b) => a - b)).toEqual([3, 5, 9]);
        expect([...phoneFirst.keys()].sort((a, b) => a - b)).toEqual([3, 5, 9]);
        for (const id of [3, 5, 9]) {
            expect(phoneFirst.get(id), `ride ${id}'s stamp`).toBe(desktopFirst.get(id));
        }
    });
});

describe("what ships", () => {
    it("keeps the simulated device out of the app", () => {
        // `loopback.ts` is a whole device — an object store, a catalog, id assignment. It exists so
        // C4, C5 and the desktop path can be built before silicon, and it has no business in a
        // visitor's tab. `platform/bundle.test.ts` guards the host split against the emitted
        // chunks; this guards the one import that would drag a mock into them.
        const src = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
        const offenders: string[] = [];
        const walk = (dir: string): void => {
            for (const entry of readdirSync(dir, { withFileTypes: true })) {
                const path = join(dir, entry.name);
                if (entry.isDirectory()) walk(path);
                else if (/\.(ts|svelte)$/.test(entry.name) && !entry.name.endsWith(".test.ts")) {
                    if (/from\s+["'][^"']*usb\/loopback["']|["'][^"']*\.\/loopback["']/.test(readFileSync(path, "utf8"))) {
                        offenders.push(relative(src, path));
                    }
                }
            }
        };
        walk(src);
        expect(offenders).toEqual([]);
    });
});

describe("the rideList fixture's shape", () => {
    /** `rideList` has no checked-in vector of its own, so pin the entry length the header carries
     *  against the two lists that do — the per-list `entry_len` is the whole reason they differ. */
    it("keeps the three list types on their own entry lengths", () => {
        expect(decodeRouteList(vector("route-list.bin")).header.entryLen).toBe(84);
        expect(decodeTripList(vector("trip-list.bin")).header.entryLen).toBe(76);
        // A rideList entry is 72 bytes; build one and check the decoder walks it.
        const { entries } = decodeRideList(
            new Uint8Array([2, 72, 1, 0, 1, 0, ...new Uint8Array(72)]),
        );
        expect(entries).toHaveLength(1);
    });
});
