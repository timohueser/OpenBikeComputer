import { IDBFactory } from 'fake-indexeddb';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { loadFlatDevice } from '../../../test-support/flat-device/load';
import { rideKeyAt } from '../library/import';
import type { RouteRecord } from '../library/records';
import { Library } from '../library/store';
import { sentKey, summarize } from '../library/summary';
import { emptyTrip, type DrawnCoordinate, type RoutePoint, type Trip } from '../planner/editor';
import { newPlan, type Plan } from '../planner/library';
import { DeviceError, type FlatStoreClient } from '../usb/client';
import { flatDevice, streamCeilingFor } from '../usb/flat-device';
import { encodeRideObject, type RideObject } from '../usb/objects';
import { ObjectKind, type CatalogEntry } from '../usb/protocol';
import { archivedRows } from './metadata';
import { rideRecord } from './ride';
import { rememberCard, remembered } from './remembered.svelte';
import { CABLE_OUT, MAP_LOST, NO_ROOM, sendMap, syncDevice, type SyncClient } from './sync';
import { tripKey, tripObject } from './trip';

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

/** A point at `lon` on the parallel 47 at the end of a drawn leg from `from`, so no routing service is asked. */
function point(id: string, kind: RoutePoint['kind'], lon: number, extra: Partial<RoutePoint> = {}, from = lon - .05): RoutePoint {
    const drawn: DrawnCoordinate[] = [[from, 47, 400], [(from + lon) / 2, 47, 420], [lon, 47, 410]];
    return { id, kind, label: id, coordinate: [lon, 47], ...kind !== 'start' && { leg: 'drawn', drawn }, ...extra };
}

/** A route of one day with two stops. */
function plan(finish = 8.15, leg?: RoutePoint['leg']): Plan {
    const trip: Trip = { ...emptyTrip('route'), days: 1, routeOrder: ['Camp', 'Spring'], points: [
        point('start', 'start', 8),
        point('Camp', 'waypoint', 8.05, { placeKind: 'campsite' }),
        point('Spring', 'waypoint', 8.1, { placeKind: 'drinking_water' }),
        point('finish', 'finish', finish, leg && { leg }),
    ] };
    const summary = { ...summarize([[8, 47, 400], [finish, 47, 410]], { lengthM: 11_000, climbM: 20, seconds: 2400, bike: 'touring' as const }), days: 1 };
    return { ...newPlan(trip, 'Black Forest'), summary };
}

/** A trip of `count` riding days along the parallel 47 from 8°E, one drawn leg of 0.05° (about 3.8 km) for each day. The
 * nights of `kept` are kept at their day ends, and `stop` names a stop in the middle of day 5. */
function longTrip(count: number, kept = Array.from({ length: count - 1 }, (_, i) => i + 1), stop?: string): Plan {
    const points = [point('start', 'start', 8)];
    const add = (id: string, kind: RoutePoint['kind'], lon: number, extra: Partial<RoutePoint> = {}) =>
        points.push(point(id, kind, lon, extra, points.at(-1)!.coordinate[0]));
    for (let k = 1; k <= count; k++) {
        if (k === 5 && stop) add('stop', 'waypoint', 8.225, { label: stop });
        const lon = 8 + .05 * k;
        if (k === count) add('finish', 'finish', lon);
        else if (kept.includes(k)) add(`night-${k}`, 'night', lon, { night: k, label: `Camp ${k}` });
        else add(`via-${k}`, 'via', lon);
    }
    const trip: Trip = { ...emptyTrip('trip'), days: count, daysSet: true, routeOrder: points.slice(1, -1).map(p => p.id), points };
    const summary = { ...summarize([[8, 47, 400], [8 + .05 * count, 47, 410]], { lengthM: count * 3800, climbM: 20, seconds: count * 900, bike: 'touring' as const }), days: count };
    return { ...newPlan(trip, 'Alps'), summary };
}

/** The route ids that a trip object names, in ride order. */
function tripDays(bytes: Uint8Array): bigint[] {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    return Array.from({ length: view.getUint16(2, true) }, (_, i) => view.getBigUint64(64 + 16 * i, true));
}

/** `client` with some of its calls replaced. */
const wrapped = (client: SyncClient, calls: Partial<SyncClient>): SyncClient => ({ list: options => client.list(options), get: (...args) => client.get(...args),
    remove: (...args) => client.remove(...args), archiveRide: (...args) => client.archiveRide(...args), put: (...args) => client.put(...args), ...calls });

/** A trip of the phone named "Alps", with a random key and two routes named as days of the planner: no sync may touch them. */
function seedPhoneTrip(rig: ReturnType<typeof flatDevice>): CatalogEntry[] {
    const days = [1, 2].map(k => rig.device.seed({ kind: ObjectKind.Route, displayName: `Day ${k} Camp ${k}`, bytes: new Uint8Array(64).fill(k) }));
    const trip = tripObject(0x5eed_0bc1_7a2e_9d43n, 'Alps', 0, days.map(day => ({ route: day.objectId, joinM: 0, leaveM: 0xffff_ffff })));
    return [...days, rig.device.seed({ kind: ObjectKind.Trip, displayName: 'Alps', bytes: trip })];
}

/** The routes and trips on the card that are not of `phone`. */
const mine = (rig: ReturnType<typeof flatDevice>, phone: CatalogEntry[]) => rig.device.entries
    .filter(entry => (entry.kind === ObjectKind.Route || entry.kind === ObjectKind.Trip) && !phone.some(other => other.objectId === entry.objectId));

function expectUntouched(rig: ReturnType<typeof flatDevice>, phone: CatalogEntry[]): void {
    expect(rig.device.entries.filter(entry => phone.some(other => other.objectId === entry.objectId)).map(entry => [entry.objectId, entry.revision]))
        .toEqual(phone.map(entry => [entry.objectId, entry.revision]));
}

function setup(storeId = CARD, extents?: number) {
    const rig = flatDevice({ storeId, streamCeiling: streamCeilingFor(512), extents });
    const library = new Library(new IDBFactory());
    const sync = (client: Parameters<typeof syncDevice>[0] = rig.client) => syncDevice(client, SERIAL, library, new AbortController().signal);
    const seedRide = (object: RideObject) => rig.device.seed({ kind: ObjectKind.Ride, displayName: 'ride', bytes: encodeRideObject(object) });
    const routes = () => rig.device.entries.filter(entry => entry.kind === ObjectKind.Route);
    const trips = () => rig.device.entries.filter(entry => entry.kind === ObjectKind.Trip);
    const send = async (route: RouteRecord) => {
        await library.saveRoute(route);
        await library.changeState('waiting', ids => [...ids ?? [], route.id]);
    };
    return { rig, library, sync, seedRide, routes, trips, send };
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
        expect(await sync()).toMatchObject({ sent: 1, trips: 0, waypointsLeft: 0, problem: null });
        const [first] = routes();
        expect(first.displayName).toBe('Black Forest');
        // The stops, each with its category (`specs/obcr-route.md` §4).
        const obcr = rig.device.payloadOf(first.objectId)!, view = new DataView(obcr.buffer, obcr.byteOffset);
        const table = view.getUint32(112, true), waypoints = Array.from({ length: view.getUint16(116, true) }, (_, i) => table + i * 80);
        expect(waypoints.map(at => [new TextDecoder().decode(obcr.subarray(at + 20, at + 20 + obcr[at + 15])), obcr[at + 14]]))
            .toEqual([['Camp', 2], ['Spring', 1]]);
        expect(await library.state('sent')).toEqual({ [route.id]: { key: sentKey(route), card: CARD, days: [`${first.objectId}`], trip: null, tripKey: null } });
        expect(await library.state('waiting')).toEqual([]);

        const changed = { ...(await library.route(route.id))!, trip: plan(8.2).trip };
        await send(await library.saveRoute(changed));
        await sync();
        expect(routes()).toMatchObject([{ objectId: first.objectId }]);
        expect(routes()[0].revision).toBeGreaterThan(first.revision);
        // A new name leaves the line as it is, and still makes the device file old.
        const renamed = await library.renameRoute(route.id, 'Black Forest loop');
        expect((await library.state('sent'))![route.id].key).not.toBe(sentKey(renamed));

        // The rider deletes the route on the device: the next sync forgets that it was sent.
        await rig.client.remove(routes()[0]);
        await sync();
        expect(await library.state('sent')).toEqual({});
        await send((await library.route(route.id))!);
        await sync();
        const [again] = routes();
        expect(again.objectId).not.toBe(first.objectId);
        expect((await library.state('sent'))![route.id].days).toEqual([`${again.objectId}`]);

        await library.removeRoute(route.id);
        expect(await sync()).toMatchObject({ removed: 1, tried: [route.id] });
        expect([routes(), await library.state('sent'), await library.state('leaving')]).toEqual([[], {}, []]);
        await rig.close();
    });

    it('removes a route that the rider deleted in the library during its first upload', async () => {
        const { rig, library, sync, routes, send } = setup();
        rememberCard(SERIAL, CARD);
        const route = plan();
        await send(route);
        const client = rig.client, deleting: Parameters<typeof syncDevice>[0] = {
            list: options => client.list(options), get: (...args) => client.get(...args), remove: (...args) => client.remove(...args),
            archiveRide: (...args) => client.archiveRide(...args),
            put: async (...args) => { const answer = client.put(...args); await library.removeRoute(route.id); return answer; },
        };
        await sync(deleting);
        expect([routes(), await library.state('leaving')]).toMatchObject([[{ displayName: 'Black Forest' }], [route.id]]);
        expect(await sync()).toMatchObject({ removed: 1 });
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
        expect(phone.map(entry => `${entry.objectId}`)).toEqual(Object.values((await library.state('sent'))!).map(route => route.days[0]));
        await library.removeRoute(deleted.id);
        await library.changeState('waiting', ids => [...ids ?? [], kept.id]);
        const report = await syncDevice(other.rig.client, SERIAL, library, new AbortController().signal);
        expect(report).toMatchObject({ problem: null, sent: 1, removed: 0 });
        const [first, second, created] = other.routes();
        expect([first, second]).toMatchObject(phone.map(entry => ({ objectId: entry.objectId, revision: entry.revision })));
        expect([await library.state('sent'), await library.state('leaving')]).toEqual([{ [kept.id]: { key: sentKey(kept), card: OTHER_CARD, days: [`${created.objectId}`], trip: null, tripKey: null } }, []]);
        await other.rig.close();
    });

    it('sends a plan of several days as a route for each day and a trip object, sends nothing more when it did not change, and removes all when deleted', async () => {
        const { rig, library, sync, routes, trips, send } = setup();
        rememberCard(SERIAL, CARD);
        const alps = longTrip(10);
        await send(alps);
        expect(await sync()).toMatchObject({ sent: 0, trips: 1, problem: null });
        const days = routes(), [trip] = trips();
        expect(days.map(day => day.displayName)).toEqual([...Array.from({ length: 9 }, (_, i) => `Day ${i + 1} Camp ${i + 1}`), 'Day 10']);
        expect([trip.displayName, tripDays(rig.device.payloadOf(trip.objectId)!)]).toEqual(['Alps', days.map(day => day.objectId)]);
        expect(await library.state('sent')).toEqual({ [alps.id]: { key: sentKey(alps), card: CARD, days: days.map(day => `${day.objectId}`), trip: `${trip.objectId}`,
            tripKey: `${tripKey(alps.id, alps.trip)}` } });

        const put = vi.spyOn(rig.client, 'put');
        await send(alps);
        expect(await sync()).toMatchObject({ trips: 1, problem: null });
        expect(put).not.toHaveBeenCalled();

        await library.removeRoute(alps.id);
        expect(await sync()).toMatchObject({ removed: 0, removedTrips: 1, tried: [alps.id] });
        expect([routes(), trips(), await library.state('sent'), await library.state('leaving')]).toEqual([[], [], {}, []]);
        await rig.close();
    });

    it('with nights 4 and 5 kept, replaces only the route of day 5 when a stop of day 5 changes', async () => {
        const { rig, library, sync, routes, trips, send } = setup();
        rememberCard(SERIAL, CARD);
        const alps = longTrip(10, [4, 5], 'Spring');
        await send(alps);
        await sync();
        expect(routes().map(day => day.displayName)).toEqual(['Day 1', 'Day 2', 'Day 3', 'Day 4 Camp 4', 'Day 5 Camp 5', 'Day 6', 'Day 7', 'Day 8', 'Day 9', 'Day 10']);
        const before = [...routes(), ...trips()], put = vi.spyOn(rig.client, 'put');
        await send(await library.saveRoute({ ...alps, trip: { ...alps.trip, points: alps.trip.points.map(p => p.id === 'stop' ? { ...p, label: 'Well' } : p) } }));
        expect(await sync()).toMatchObject({ trips: 1, problem: null });
        // The trip object names the same routes, so it stays as it is.
        expect(put).toHaveBeenCalledTimes(1);
        expect([...routes(), ...trips()].map((entry, i) => entry.revision !== before[i].revision)).toEqual(before.map((_, i) => i === 4));
        await rig.close();
    });

    it('sends nothing of a plan when the menu of the device has no room for its days', async () => {
        const { rig, library, sync, routes, trips, send } = setup();
        rememberCard(SERIAL, CARD);
        for (let i = 0; i < 60; i++) rig.device.seed({ kind: ObjectKind.Route, displayName: 'Phone', bytes: new Uint8Array(64) });
        const alps = longTrip(10);
        await send(alps);
        expect((await sync()).problem).toBe('“Alps” waits: the bike computer has room for 4 more routes, it needs 10. Remove ridden or unused routes on the bike computer.');
        expect([routes().length, trips().length, await library.state('waiting'), await library.state('sent')]).toEqual([60, 0, [alps.id], undefined]);
        await rig.close();
    });

    it('after a cut cable sends only the rest, and keeps the copy that a lost answer left', async () => {
        const { rig, library, sync, routes, trips, send } = setup();
        rememberCard(SERIAL, CARD);
        const alps = longTrip(10), client = rig.client;
        await send(alps);
        let puts = 0;
        // The sixth put reaches the card, and its answer is lost.
        const cutting = wrapped(client, { put: async (...args) => {
            const answer = await client.put(...args);
            if (++puts === 6) throw new DeviceError('link', 'gone');
            return answer;
        } });
        expect(await sync(cutting)).toMatchObject({ trips: 0, problem: CABLE_OUT });
        expect([routes().length, (await library.state('sent'))![alps.id].days.filter(Boolean).length]).toEqual([6, 5]);
        const put = vi.spyOn(client, 'put');
        expect(await sync()).toMatchObject({ trips: 1, problem: null });
        // Days 7 to 10 and the trip object.
        expect([put.mock.calls.length, routes().length, trips().length]).toEqual([5, 10, 1]);
        await rig.close();
    });

    it('removes the trip and days 2 to 10 of a plan that becomes one day, keeps day 1 as the route, and back again', async () => {
        const { rig, library, sync, routes, trips, send } = setup();
        rememberCard(SERIAL, CARD);
        const alps = longTrip(10);
        await send(alps);
        await sync();
        const [first, ...rest] = routes(), [trip] = trips(), remove = vi.spyOn(rig.client, 'remove');
        const one = await library.saveRoute({ ...alps, trip: { ...alps.trip, mode: 'route' } });
        await send(one);
        expect(await sync()).toMatchObject({ sent: 1, trips: 0, problem: null });
        expect(remove.mock.calls.map(([entry]) => entry.objectId)).toEqual([trip, ...rest.reverse()].map(entry => entry.objectId));
        expect([routes(), trips()]).toMatchObject([[{ objectId: first.objectId, displayName: 'Alps' }], []]);

        await send(await library.saveRoute({ ...one, trip: alps.trip }));
        expect(await sync()).toMatchObject({ sent: 0, trips: 1, problem: null });
        expect([routes()[0].objectId, routes().length, trips().length]).toEqual([first.objectId, 10, 1]);
        await rig.close();
    });

    it('starts the route of a day after a train where the train arrives', async () => {
        const { rig, sync, routes, send } = setup();
        rememberCard(SERIAL, CARD);
        const alps = longTrip(2), arrival: RoutePoint = { id: 'arrival', kind: 'via', label: 'Station', coordinate: [8.3, 47.2], leg: 'transfer' };
        const finish: RoutePoint = { ...point('finish', 'finish', 8.35), coordinate: [8.35, 47.2], drawn: [[8.3, 47.2, 400], [8.35, 47.2, 410]] };
        await send({ ...alps, trip: { ...alps.trip, routeOrder: ['night-1', 'arrival'], points: [...alps.trip.points.slice(0, 2), arrival, finish] } });
        expect(await sync()).toMatchObject({ trips: 1, problem: null });
        const obcr = rig.device.payloadOf(routes()[1].objectId)!, view = new DataView(obcr.buffer, obcr.byteOffset);
        // The start point of the header (`specs/obcr-route.md` §1).
        expect([view.getInt32(24, true), view.getInt32(28, true)]).toEqual([8_300_000, 47_200_000]);
        await rig.close();
    });

    it('forgets a trip that the rider deleted on the device with its routes, and sends it again', async () => {
        const { rig, library, sync, routes, trips, send } = setup();
        rememberCard(SERIAL, CARD);
        const alps = longTrip(10);
        await send(alps);
        await sync();
        // A delete of a trip on the device removes the trip and its routes.
        for (const entry of [...trips(), ...routes()]) await rig.client.remove(entry);
        await sync();
        expect(await library.state('sent')).toEqual({});
        await send(alps);
        expect(await sync()).toMatchObject({ trips: 1, problem: null });
        expect([routes().length, trips().length]).toEqual([10, 1]);
        await rig.close();
    });

    it('keeps the objects that are left when the rider deletes a day on the device: a new send and a delete leave no loose route', async () => {
        const { rig, library, sync, routes, trips, send } = setup();
        rememberCard(SERIAL, CARD);
        const phone = seedPhoneTrip(rig), alps = longTrip(10);
        await send(alps);
        await sync();
        const ids = mine(rig, phone).map(entry => `${entry.objectId}`);
        await rig.client.remove(routes().find(entry => entry.displayName === 'Day 4 Camp 4')!);
        await sync();
        // The plan is not on the device, and the entry still names the trip and the other days.
        expect((await library.state('sent'))![alps.id]).toEqual({ key: '', card: CARD, days: ids.slice(0, 10).map((id, i) => i === 3 ? '' : id), trip: ids[10],
            tripKey: `${tripKey(alps.id, alps.trip)}` });

        // Another bike type changes every day, so no copy matches by its content: the days are replaced in place, and day 4 is made again.
        const put = vi.spyOn(rig.client, 'put');
        await send(await library.saveRoute({ ...alps, trip: { ...alps.trip, bike: 'gravel' } }));
        expect(await sync()).toMatchObject({ trips: 1, problem: null });
        expect(put.mock.calls.map(([target]) => target.objectId === undefined)).toEqual([false, false, false, true, false, false, false, false, false, false, false]);
        expect([mine(rig, phone).length, trips().length]).toEqual([11, 2]);

        // The rider deletes day 7 on the device, then the plan in the library.
        await rig.client.remove(routes().find(entry => entry.displayName === 'Day 7 Camp 7')!);
        await sync();
        await library.removeRoute(alps.id);
        expect(await sync()).toMatchObject({ removed: 0, removedTrips: 1, problem: null });
        expect([mine(rig, phone), await library.state('sent')]).toEqual([[], {}]);
        expectUntouched(rig, phone);
        await rig.close();
    });

    it('finds a trip object whose put answer was lost: a rename replaces it, a delete removes it', async () => {
        const { rig, library, sync, trips, send } = setup();
        rememberCard(SERIAL, CARD);
        const phone = seedPhoneTrip(rig), client = rig.client;
        // The trip object reaches the card, and its answer is lost.
        const lossy = wrapped(client, { put: async (target, ...rest) => {
            const answer = await client.put(target, ...rest);
            if (target.kind === ObjectKind.Trip) throw new DeviceError('link', 'gone');
            return answer;
        } });
        const alps = longTrip(10), rhine = { ...longTrip(10), name: 'Rhine' };
        await send(alps);
        expect(await sync(lossy)).toMatchObject({ problem: CABLE_OUT });
        const [lost] = mine(rig, phone).filter(entry => entry.kind === ObjectKind.Trip);
        expect((await library.state('sent'))![alps.id].trip).toBeNull();
        // The plan still waits.
        await library.renameRoute(alps.id, 'Alps north');
        expect(await sync()).toMatchObject({ trips: 1, problem: null });
        expect(mine(rig, phone).filter(entry => entry.kind === ObjectKind.Trip)).toMatchObject([{ objectId: lost.objectId, displayName: 'Alps north' }]);

        await send(rhine);
        expect(await sync(lossy)).toMatchObject({ problem: CABLE_OUT });
        await library.removeRoute(rhine.id);
        expect(await sync()).toMatchObject({ removedTrips: 1, problem: null });
        // Only the trip and the 10 days of "Alps north" are left of the planner.
        expect([mine(rig, phone).length, trips().map(entry => entry.displayName).sort()]).toEqual([11, ['Alps', 'Alps north']]);
        expectUntouched(rig, phone);
        await rig.close();
    });

    it('never takes a trip of the phone that lists a day route of the plan, nor that route', async () => {
        const { rig, library, sync, routes, send } = setup();
        rememberCard(SERIAL, CARD);
        const alps = longTrip(10), client = rig.client;
        await send(alps);
        // The cable comes out before the trip object goes.
        const cutting = wrapped(client, { put: (target, ...rest) => target.kind === ObjectKind.Trip ? Promise.reject(new DeviceError('link', 'gone')) : client.put(target, ...rest) });
        expect(await sync(cutting)).toMatchObject({ problem: CABLE_OUT });
        // The phone takes the route of day 2 into a trip of its own, by its content.
        const day2 = routes()[1], bytes = tripObject(0x5eed_0bc1_7a2e_9d43n, 'Alps', 0, [{ route: day2.objectId, joinM: 0, leaveM: 0xffff_ffff }]);
        const phone = [day2, rig.device.seed({ kind: ObjectKind.Trip, displayName: 'Alps', bytes })];
        // The plan still waits.
        expect(await sync()).toMatchObject({ trips: 1, problem: null });
        await library.removeRoute(alps.id);
        expect(await sync()).toMatchObject({ removedTrips: 1, problem: null });
        expect(mine(rig, phone)).toEqual([]);
        expectUntouched(rig, phone);
        await rig.close();
    });

    it('makes room before it makes a day again when the menu of the device is full', async () => {
        // A card with room for more objects than the menu shows.
        const { rig, library, sync, routes, send } = setup(CARD, 256);
        rememberCard(SERIAL, CARD);
        const rides = Array.from({ length: 54 }, (_, i) => rig.device.seed({ kind: ObjectKind.Route, displayName: `Ride ${i}`, bytes: new Uint8Array(64).fill(i) }));
        const alps = longTrip(10), client = rig.client;
        await send(alps);
        await sync();
        await client.remove(routes().find(entry => entry.displayName === 'Day 4 Camp 4')!);
        await sync();
        rig.device.seed({ kind: ObjectKind.Route, displayName: 'Ride 54', bytes: new Uint8Array(64).fill(54) });
        // The plan loses its last day, and the cable comes out right after day 4 is made again.
        const cutting = wrapped(client, { put: async (target, ...rest) => {
            const answer = await client.put(target, ...rest);
            if (target.objectId === undefined) throw new DeviceError('link', 'gone');
            return answer;
        } });
        await send({ ...longTrip(9), id: alps.id });
        expect(await sync(cutting)).toMatchObject({ problem: CABLE_OUT });
        // The menu of the device shows the newest 64 routes.
        expect(routes().slice(-64).map(entry => entry.objectId)).toContain(rides[0].objectId);
        expect(await library.state('waiting')).toEqual([alps.id]);
        await rig.close();
    });

    it('names only the kept days in the trip object before it removes the days that a shorter trip no longer has', async () => {
        const { rig, library, sync, routes, trips, send } = setup();
        rememberCard(SERIAL, CARD);
        const alps = longTrip(10), client = rig.client;
        await send(alps);
        await sync();
        const ids = routes().map(entry => entry.objectId);
        // The device refuses to remove day 7, as for a route that a ride holds. The plan keeps its id and its first two points,
        // so its trip key.
        const refusing = wrapped(client, { remove: (ref, signal) => ref.objectId === ids[6] ? Promise.reject(new DeviceError('busy', 'The route is in use.')) : client.remove(ref, signal) });
        await send({ ...longTrip(6), id: alps.id });
        expect((await sync(refusing)).problem).toBe('“Alps” waits: The route is in use.');
        expect([tripDays(rig.device.payloadOf(trips()[0].objectId)!), routes().map(entry => entry.objectId)]).toEqual([ids.slice(0, 6), ids.slice(0, 7)]);
        expect(await library.state('waiting')).toEqual([alps.id]);
        await rig.close();
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

    it('keeps a route that waits when a leg of its line has no legal path, and says so', async () => {
        const { rig, library, sync, routes, send } = setup();
        rememberCard(SERIAL, CARD);
        vi.spyOn(globalThis, 'fetch').mockResolvedValue({ ok: false, json: async () => ({ code: 'no_path', message: 'No legal route.', leg: 0 }) } as Response);
        const route = plan(8.15, 'routed');
        await send(route);
        const report = await sync();
        expect([report.sent, report.problem]).toEqual([0, '“Black Forest” waits: 1 leg has no legal path. Change it in the planner.']);
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

describe('rideRecord', () => {
    it('keeps the trip of the footer, with the key in decimal and the day from 1', () => {
        const trip = { key: 2n ** 63n + 5n, dayIndex: 1, dayCount: 3, name: 'Alps' };
        expect(rideRecord(ride(1_780_000_000, { trip }), 'card:1')?.trip).toEqual({ key: '9223372036854775813', day: 2, name: 'Alps' });
        expect(rideRecord(ride(1_780_000_000), 'card:1')).not.toHaveProperty('trip');
    });
});

describe('sendMap', () => {
    const next = { regions: ['europe/switzerland'], boxes: [], light: 'default', dark: 'dusk', bytes: 870_000_000 };
    const job = (controller = new AbortController()) =>
        ({ signal: controller.signal, cancel: (reason?: unknown) => controller.abort(reason), phase() {}, progress() {} });

    async function waiting() {
        const library = new Library(new IDBFactory());
        rememberCard(SERIAL, CARD);
        await library.setState('nextMap', next);
        return library;
    }

    it('remembers the area of the sent map, and the map waits no more unless the rider chose another one during the build', async () => {
        const library = await waiting();
        // The build gives the versions of the styles that it sent, which can be newer than those of the map that waited.
        const area = { regions: ['europe/switzerland'], boxes: [], light: 'default', dark: 'dusk', lightVersion: 3, darkVersion: 1 };
        expect(await sendMap(SERIAL, library, async () => ({ objectId: 7n, revision: 2n, area }), job())).toBeNull();
        expect(remembered.device).toMatchObject({ mapObject: '7:2', built: { object: '7:2', ...area } });
        expect(await library.state('nextMap')).toBeNull();

        const other = { ...next, dark: 'custom-night' };
        await library.setState('nextMap', next);
        const build = async () => {
            await library.setState('nextMap', other);
            return { objectId: 7n, revision: 3n, area };
        };
        expect(await sendMap(SERIAL, library, build, job())).toBeNull();
        expect(remembered.device?.built).toMatchObject({ object: '7:3', dark: 'dusk' });
        expect(await library.state('nextMap')).toEqual(other);
    });

    it('keeps the map that waits and says why: a card with no room, a cut cable, a stop', async () => {
        const library = await waiting();
        const fail = (error: unknown) => async () => { throw error; };
        expect(await sendMap(SERIAL, library, fail(new DeviceError('no-space', 'full')), job())).toBe(NO_ROOM);
        expect(await sendMap(SERIAL, library, fail(new DeviceError('link', 'gone')), job())).toBe(MAP_LOST);
        const stop = new AbortController();
        stop.abort();
        expect(await sendMap(SERIAL, library, fail(stop.signal.reason), job(stop))).toBe(MAP_LOST);
        expect(await sendMap(SERIAL, library, fail(new Error('This area cannot be built: no data.')), job())).toBe('This area cannot be built: no data.');
        expect(await library.state('nextMap')).toEqual(next);
        expect(remembered.device?.built).toBeNull();
    });
});
