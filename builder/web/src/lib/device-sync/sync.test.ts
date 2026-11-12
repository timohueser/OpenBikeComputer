import { IDBFactory } from 'fake-indexeddb';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { loadFlatDevice } from '../../../test-support/flat-device/load';
import { rideKeyAt } from '../library/import';
import type { RouteRecord } from '../library/records';
import { Library } from '../library/store';
import { lineKey, summarize } from '../library/summary';
import { emptyTrip, type DrawnCoordinate, type RoutePoint, type Trip } from '../planner/editor';
import { newPlan, type Plan } from '../planner/library';
import { DeviceError, type FlatStoreClient } from '../usb/client';
import { flatDevice, streamCeilingFor } from '../usb/flat-device';
import { encodeRideObject, type RideObject } from '../usb/objects';
import { ObjectKind } from '../usb/protocol';
import { archivedRows } from './metadata';
import { rememberCard, remembered } from './remembered.svelte';
import { CABLE_OUT, syncDevice } from './sync';

beforeAll(loadFlatDevice);
beforeEach(() => { remembered.device = null; });
afterEach(() => vi.restoreAllMocks());

const SERIAL = '9F3A00C1E2B47D10', CARD = '8f2c41d96b074ea3b1559c207de83466', OTHER_CARD = '11112222333344445555666677778888';

function ride(start: number, extra: Partial<RideObject> = {}, points = 3): RideObject {
    return {
        version: 6, isDemo: false, name: '', startTime: start, distanceM: 0, movingTimeS: 0, avgSpeedCms: 0, climbM: 0, descentM: 0,
        avgHr: null, maxHr: null, avgCadence: null, avgPower: null, maxPower: null, energyKj: null, bikeType: 1, trip: null,
        effortLimits: { maxHrBpm: 186, ftpW: 240 },
        // The clock of the device wraps during this ride.
        points: Array.from({ length: points }, (_, i) => ({ tMs: (0xffff_f000 + i * 2000) >>> 0, latMicrodegrees: 47_000_000 + i * 100,
            lonMicrodegrees: 8_000_000 + i * 100, elevationM: 400 + i, segmentStart: i === 0, hrBpm: 120, cadenceRpm: null, powerW: null })),
        ...extra,
    };
}

/** A point at `lon` on the parallel 47 at the end of a drawn leg, so no routing service is asked. */
function point(id: string, kind: RoutePoint['kind'], lon: number, extra: Partial<RoutePoint> = {}): RoutePoint {
    const drawn: DrawnCoordinate[] = [[lon - .05, 47, 400], [lon - .025, 47, 420], [lon, 47, 410]];
    return { id, kind, label: id, coordinate: [lon, 47], ...kind !== 'start' && { leg: 'drawn', drawn }, ...extra };
}

function plan(finish = 8.15, leg?: RoutePoint['leg']): Plan {
    const trip: Trip = { ...emptyTrip('trip'), days: 2, daysSet: true, routeOrder: ['night-1', 'Spring'], points: [
        point('start', 'start', 8),
        point('night-1', 'night', 8.05, { night: 1, label: 'Camp', placeKind: 'campsite' }),
        point('Spring', 'waypoint', 8.1, { placeKind: 'drinking_water' }),
        point('finish', 'finish', finish, leg && { leg }),
    ] };
    const summary = { ...summarize([[8, 47, 400], [finish, 47, 410]], { lengthM: 11_000, climbM: 20, seconds: 2400, bike: 'touring' as const }), days: 2 };
    return { ...newPlan(trip, 'Black Forest'), summary };
}

function setup(storeId = CARD) {
    const rig = flatDevice({ storeId, streamCeiling: streamCeilingFor(512) });
    const library = new Library(new IDBFactory());
    const sync = (client: Parameters<typeof syncDevice>[0] = rig.client) => syncDevice(client, SERIAL, library, new AbortController().signal);
    const seedRide = (object: RideObject) => rig.device.seed({ kind: ObjectKind.Ride, displayName: 'ride', bytes: encodeRideObject(object) });
    const routes = () => rig.device.entries.filter(entry => entry.kind === ObjectKind.Route);
    const send = async (route: RouteRecord) => {
        await library.saveRoute(route);
        await library.changeState('waiting', ids => [...ids ?? [], route.id]);
    };
    return { rig, library, sync, seedRide, routes, send };
}

describe('syncDevice', () => {
    it('takes each new ride once, keeps deleted and demo rides out, and counts what came in today', async () => {
        const { rig, library, sync, seedRide } = setup();
        rememberCard(SERIAL, CARD);
        seedRide(ride(1_780_000_000));
        const demo = seedRide(ride(1_780_050_000, { isDemo: true }));
        seedRide(ride(1_780_100_000));
        // A ride that the device still records cannot be fetched: a `get` of it would give a problem.
        rig.device.seed({ kind: ObjectKind.Ride, displayName: 'recording' });
        expect(await sync()).toMatchObject({ rides: 2, problem: null });
        const keys = (await library.rides()).map(r => r.key).sort();
        expect(keys).toEqual([rideKeyAt(1_780_000_000), rideKeyAt(1_780_100_000)]);
        expect(remembered.device!.skipped).toEqual([demo.objectId.toString()]);
        const taken = (await library.ride(keys[0]))!;
        // The wrapped clock still gives four seconds from the first point to the last.
        expect(taken).toMatchObject({ source: 'device', format: 'obcride', maxHr: 186, ftp: 240, summary: { bike: 'gravel', seconds: 4 } });

        await library.removeRide(keys[1]);
        seedRide(ride(1_780_200_000));
        expect(await sync()).toMatchObject({ rides: 1 });
        expect(await sync()).toMatchObject({ rides: 0 });
        expect((await library.rides()).map(r => r.key).sort()).toEqual([keys[0], rideKeyAt(1_780_200_000)]);
        expect(remembered.device!.came).toMatchObject({ count: 3 });
        await rig.close();
    });

    it('marks only stored rides whose file is the exact object, and only with persistent storage', async () => {
        const { rig, library, sync, seedRide } = setup();
        rememberCard(SERIAL, CARD);
        const persist = vi.spyOn(library, 'persist').mockResolvedValue(false);
        const first = seedRide(ride(1_780_000_000));
        expect(await sync()).toMatchObject({ rides: 1, marked: 0 });
        expect(rig.device.entries.some(entry => entry.kind === ObjectKind.Metadata)).toBe(false);

        // The same ride came from a file first, so the library does not hold the bytes of the device.
        const fromFile = { ...(await library.ride(rideKeyAt(1_780_000_000)))!, key: rideKeyAt(1_780_300_000), source: 'file' as const, device: undefined };
        await library.addRide({ ...fromFile, format: 'gpx' }, new Blob(['<gpx/>']));
        seedRide(ride(1_780_300_000));
        // The rider deleted this ride, so the library rejects it and the device keeps it as not taken.
        await library.addRide({ ...fromFile, key: rideKeyAt(1_780_400_000) }, new Blob(['<gpx/>']));
        await library.removeRide(rideKeyAt(1_780_400_000));
        seedRide(ride(1_780_400_000));
        persist.mockResolvedValue(true);
        expect(await sync()).toMatchObject({ rides: 0, marked: 1 });
        const metadata = rig.device.entries.find(entry => entry.kind === ObjectKind.Metadata)!;
        expect(archivedRows(rig.device.payloadOf(metadata.objectId)!).map(row => row.objectId)).toEqual([first.objectId]);
        expect(await sync()).toMatchObject({ marked: 0 });
        await rig.close();
    });

    it('sends a route that waits, replaces it when it changed, creates it again when it is gone, and removes a deleted one', async () => {
        const { rig, library, sync, routes, send } = setup();
        rememberCard(SERIAL, CARD);
        const route = plan();
        await send(route);
        expect(await sync()).toMatchObject({ sent: 1, waypointsLeft: 0, problem: null });
        const [first] = routes();
        expect(first.displayName).toBe('Black Forest');
        // The night, then the stop, each with its category (`specs/obcr-route.md` §4).
        const obcr = rig.device.payloadOf(first.objectId)!, view = new DataView(obcr.buffer, obcr.byteOffset);
        const table = view.getUint32(112, true), waypoints = Array.from({ length: view.getUint16(116, true) }, (_, i) => table + i * 80);
        expect(waypoints.map(at => [new TextDecoder().decode(obcr.subarray(at + 20, at + 20 + obcr[at + 15])), obcr[at + 14]]))
            .toEqual([['Camp', 2], ['Spring', 1]]);
        expect(await library.state('sent')).toEqual({ [route.id]: { device: `${CARD}:${first.objectId}`, line: lineKey(route) } });
        expect(await library.state('waiting')).toEqual([]);

        const changed = { ...(await library.route(route.id))!, trip: plan(8.2).trip };
        await send(await library.saveRoute(changed));
        await sync();
        expect(routes()).toMatchObject([{ objectId: first.objectId }]);
        expect(routes()[0].revision).toBeGreaterThan(first.revision);

        await rig.client.remove(routes()[0]);
        await send((await library.route(route.id))!);
        await sync();
        const [again] = routes();
        expect(again.objectId).not.toBe(first.objectId);
        expect((await library.state('sent'))![route.id].device).toBe(`${CARD}:${again.objectId}`);

        await library.removeRoute(route.id);
        expect(await sync()).toMatchObject({ removed: 1, tried: [route.id] });
        expect([routes(), await library.state('sent'), await library.state('leaving')]).toEqual([[], {}, []]);
        await rig.close();
    });

    it('touches no object of another card that has the ObjectIds of the routes it sent', async () => {
        const { rig, library, sync, send } = setup();
        rememberCard(SERIAL, CARD);
        const kept = plan(), deleted = plan(8.2);
        await send(kept);
        await send(deleted);
        await sync();
        await rig.close();
        // The card is formatted, and the phone puts two routes with the same ObjectIds on it.
        const other = setup(OTHER_CARD), phone = [1, 2].map(() => other.rig.device.seed({ kind: ObjectKind.Route, displayName: 'Phone', bytes: new Uint8Array(64) }));
        expect(phone.map(entry => entry.objectId)).toEqual(Object.values((await library.state('sent'))!).map(route => BigInt(route.device.split(':')[1])));
        await library.removeRoute(deleted.id);
        await library.changeState('waiting', ids => [...ids ?? [], kept.id]);
        const report = await syncDevice(other.rig.client, SERIAL, library, new AbortController().signal);
        expect(report).toMatchObject({ problem: null, sent: 1, removed: 0 });
        const [first, second, created] = other.routes();
        expect([first, second]).toMatchObject(phone.map(entry => ({ objectId: entry.objectId, revision: entry.revision })));
        expect([await library.state('sent'), await library.state('leaving')]).toEqual([{ [kept.id]: { device: `${OTHER_CARD}:${created.objectId}`, line: lineKey(kept) } }, []]);
        await other.rig.close();
    });

    it('sends the routes that wait after the device refuses a ride and a mark', async () => {
        const { rig, library, sync, routes, send, seedRide } = setup();
        rememberCard(SERIAL, CARD);
        seedRide(ride(1_780_000_000));
        await sync();
        seedRide(ride(1_780_100_000));
        vi.spyOn(library, 'persist').mockResolvedValue(true);
        await send(plan());
        const client = rig.client, refusing: Parameters<typeof syncDevice>[0] = {
            list: options => client.list(options), put: (...args) => client.put(...args), remove: (...args) => client.remove(...args),
            get: (ref, options) => options?.expectedLength ? Promise.reject(new DeviceError('checksum', 'bad ride')) : client.get(ref, options),
            archiveRide: () => Promise.reject(new DeviceError('busy', 'device busy')),
        };
        const report = await sync(refusing);
        expect(report).toMatchObject({ rides: 0, marked: 0, sent: 1 });
        expect(report.problem).toMatch(/bad ride.*device busy/);
        expect(routes()).toHaveLength(1);
        await rig.close();
    });

    it('keeps a route that waits when the routing service cannot be reached, and names it', async () => {
        const { rig, library, sync, routes, send } = setup();
        rememberCard(SERIAL, CARD);
        vi.spyOn(globalThis, 'fetch').mockRejectedValue(new TypeError('offline'));
        const route = plan(8.15, 'routed');
        await send(route);
        const report = await sync();
        expect(report.sent).toBe(0);
        expect(report.problem).toContain('“Black Forest” waits');
        expect([await library.state('waiting'), routes()]).toEqual([[route.id], []]);
        await rig.close();
    });

    it('does not touch a device that is not remembered', async () => {
        const { library } = setup();
        const list = vi.fn();
        expect(await syncDevice({ list } as unknown as FlatStoreClient, SERIAL, library, new AbortController().signal)).toMatchObject({ rides: 0 });
        rememberCard('0000000000000001', CARD);
        await syncDevice({ list } as unknown as FlatStoreClient, SERIAL, library, new AbortController().signal);
        expect(list).not.toHaveBeenCalled();
    });

    it('keeps no ride when the cable comes out during its download', async () => {
        const { rig, library, sync, seedRide } = setup();
        rememberCard(SERIAL, CARD);
        seedRide(ride(1_780_000_000, {}, 400));
        const client = rig.client, cut: Parameters<typeof syncDevice>[0] = {
            list: options => client.list(options), put: (...args) => client.put(...args), remove: (...args) => client.remove(...args),
            archiveRide: (...args) => client.archiveRide(...args),
            get: (ref, options) => client.get(ref, { ...options, onProgress: done => { if (done > 0) void rig.link.device.close(); } }),
        };
        expect(await sync(cut)).toMatchObject({ rides: 0, problem: CABLE_OUT });
        expect(await library.rides()).toEqual([]);
        await rig.close().catch(() => undefined);
    });
});
