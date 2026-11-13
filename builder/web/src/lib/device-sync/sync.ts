/** One sync with the remembered device: the new rides come into the library and are marked as taken, the deleted routes
 * leave the device, and the routes that wait go to it. */
import { displayName } from '../device/write';
import type { Library } from '../library/store';
import { sentKey } from '../library/summary';
import { planTitle } from '../planner/editor';
import { isPlan } from '../planner/library';
import { LegCache } from '../planner/route-legs';
import { calculateLine } from '../planner/routing';
import { takeDeviceValues } from '../settings/settings.svelte';
import { DeviceError, type Catalog, type FlatStoreClient } from '../usb/client';
import { Crc32 } from '../usb/crc32';
import { decodeRideObject } from '../usb/objects';
import { EntryFlags, ObjectKind, type CatalogEntry } from '../usb/protocol';
import { archivedRows } from './metadata';
import { remember, rememberCard, remembered, type DeviceBytes } from './remembered.svelte';
import { rideRecord } from './ride';
import { routeObcr } from './route';

/** What a sync uses of the client. */
export type SyncClient = Pick<FlatStoreClient, 'list' | 'get' | 'put' | 'remove' | 'archiveRide'>;

export interface SyncReport {
    /** Rides that came into the library. */
    rides: number;
    /** Rides that the device now marks as taken. */
    marked: number;
    sent: number;
    removed: number;
    /** Waypoints of the sent routes past the 32 that the device reads. */
    waypointsLeft: number;
    /** The route ids that the sync tried to send or to remove. */
    tried: string[];
    /** What did not finish, in sentences for the rider; null when all finished. */
    problem: string | null;
}

export const CABLE_OUT = 'The update stopped when the cable came out. It goes on at the next connection.';

const unused = (entry: CatalogEntry) => (entry.flags & (EntryFlags.Recording | EntryFlags.Retained)) === 0;
const ofKind = (catalog: Catalog, kind: number) => catalog.entries.filter(entry => entry.kind === kind && unused(entry));
const byId = (a: CatalogEntry, b: CatalogEntry) => a.objectId < b.objectId ? -1 : a.objectId > b.objectId ? 1 : 0;
const text = (error: unknown) => error instanceof Error ? error.message : String(error);
/** The link went away, or the run was cancelled: nothing more can be done in this run. */
const cut = (error: unknown) => error instanceof DeviceError && ['link', 'timeout', 'aborted'].includes(error.code);
const refused = (error: unknown, ...codes: string[]) => error instanceof DeviceError && codes.includes(error.code);

function today(): string {
    const now = new Date(), pad = (value: number) => String(value).padStart(2, '0');
    return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/** Runs one sync with the device `serial`, only when it is the remembered device. State writes use `changeState`, so a
 * write of another tab or page is not lost. A ride is stored only after its whole `get`, and a route is `sent` only after
 * its `put`, so a cut cable loses nothing. A lost `put` answer, and then a new line from the routing, can leave a second
 * copy of a route on the device. */
export async function syncDevice(client: SyncClient, serial: string, library: Library, signal: AbortSignal): Promise<SyncReport> {
    const report: SyncReport = { rides: 0, marked: 0, sent: 0, removed: 0, waypointsLeft: 0, tried: [], problem: null };
    const problems: string[] = [];
    if (remembered.device?.serial !== serial) return report;
    try {
        const catalog = await client.list({ signal });
        rememberCard(serial, catalog.storeId);
        const card = catalog.storeId, rides = ofKind(catalog, ObjectKind.Ride).sort(byId);
        const ref = (entry: CatalogEntry) => `${card}:${entry.objectId}`;

        const known = await library.deviceRides(), skipped = new Set(remembered.device!.skipped);
        for (const entry of rides) {
            const id = entry.objectId.toString();
            if (known.has(ref(entry)) || skipped.has(id)) continue;
            let bytes;
            try { bytes = (await client.get(entry, { signal, expectedLength: Number(entry.payloadLength) })).bytes; } catch (error) {
                if (cut(error)) throw error;
                problems.push(text(error));
                continue;
            }
            let record;
            try { record = rideRecord(decodeRideObject(bytes), ref(entry)); } catch {
                problems.push('A ride on the device cannot be read by this page. Reload the page to take it.');
                continue;
            }
            if (!record) {
                skipped.add(id);
                remember({ skipped: [...skipped] });
            } else if (await library.addRide(record, new Blob([bytes as Uint8Array<ArrayBuffer>]))) {
                report.rides++;
                takeDeviceValues(record.summary.start, record.maxHr, record.ftp);
                const came = remembered.device!.came, day = today();
                remember({ came: { day, count: came?.day === day ? came.count + 1 : 1 } });
            }
        }

        // The device marks a ride as taken only while the browser keeps the library when the disk is full.
        if (await library.persist()) report.marked = await mark(client, catalog, rides, library, signal, problems);

        // A route sent to another card is not on this one: it has nothing to remove here, and a new send creates it.
        const sent = await library.state('sent') ?? {};
        for (const id of await library.state('leaving') ?? []) {
            report.tried.push(id);
            const entry = ofKind(catalog, ObjectKind.Route).find(e => ref(e) === sent[id]?.device);
            try {
                if (entry) await client.remove(entry, signal);
            } catch (error) {
                if (!refused(error, 'not-found')) { if (cut(error)) throw error; problems.push(text(error)); continue; }
            }
            await library.changeState('sent', routes => Object.fromEntries(Object.entries(routes ?? {}).filter(([other]) => other !== id)));
            await library.changeState('leaving', ids => (ids ?? []).filter(other => other !== id));
            if (entry) report.removed++;
        }

        const legs = new LegCache();
        for (const id of await library.state('waiting') ?? []) {
            report.tried.push(id);
            const route = await library.item(id);
            if (!route || !('revision' in route) || !isPlan(route)) { problems.push('A route that waits cannot be read.'); continue; }
            const name = displayName(route.name || planTitle(route.trip), 'Route');
            let file;
            try { file = await routeObcr(route, await calculateLine(route.trip, signal, legs)); } catch (error) {
                if (signal.aborted) throw error;
                problems.push(`“${name}” waits: ${text(error)}`);
                continue;
            }
            const crc = Crc32.of(file.bytes), length = BigInt(file.bytes.length), routes = ofKind(catalog, ObjectKind.Route);
            const others = await library.state('sent') ?? {}, object = others[id]?.device;
            // A lost answer of an earlier put left this copy; the copy of another route of the library is not it.
            const taken = new Set(Object.entries(others).flatMap(([other, route]) => other === id ? [] : [route.device]));
            const target = routes.find(e => ref(e) === object)
                ?? routes.find(e => e.payloadLength === length && e.payloadCrc32 === crc && e.displayName === name && !taken.has(ref(e)));
            // Without a target, the put creates an object.
            const put = (on?: CatalogEntry) =>
                client.put({ objectId: on?.objectId, expectedRevision: on?.revision, kind: ObjectKind.Route, displayName: name }, file.bytes, { signal });
            let answer;
            try {
                answer = await put(target).catch(error => target && refused(error, 'not-found') ? put() : Promise.reject(error));
            } catch (error) {
                if (cut(error)) throw error;
                problems.push(`“${name}” waits: ${text(error)}`);
                continue;
            }
            await library.changeState('sent', routes => ({ ...routes, [id]: { device: `${card}:${answer.objectId}`, key: sentKey(route) } }));
            // The rider deleted the route during its upload, after `removeRoute` looked at `sent`.
            if (!await library.item(id)) await library.changeState('leaving', ids => [...(ids ?? []).filter(other => other !== id), id]);
            await library.changeState('waiting', ids => (ids ?? []).filter(other => other !== id));
            report.sent++;
            report.waypointsLeft += file.left;
        }

        const last = await client.list({ signal });
        // A route that the device lost, or that the rider deleted on it, can be sent again.
        const on = new Set(ofKind(last, ObjectKind.Route).map(entry => `${last.storeId}:${entry.objectId}`));
        if (Object.values(await library.state('sent') ?? {}).some(route => route.device.startsWith(`${last.storeId}:`) && !on.has(route.device))) {
            await library.changeState('sent', routes => Object.fromEntries(Object.entries(routes ?? {})
                .filter(([, route]) => !route.device.startsWith(`${last.storeId}:`) || on.has(route.device))));
        }
        rememberContents(last);
    } catch (error) {
        problems.push(cut(error) || signal.aborted ? CABLE_OUT : text(error));
    }
    report.problem = problems.join(' ') || null;
    return report;
}

/** Marks each stored ride of `rides` that the Metadata object does not mark, after a check that the stored file is the exact
 * object. It stops when the device has no archive or no room for it. */
async function mark(client: SyncClient, catalog: Catalog, rides: CatalogEntry[], library: Library, signal: AbortSignal,
    problems: string[]): Promise<number> {
    const metadata = ofKind(catalog, ObjectKind.Metadata)[0];
    let rows;
    try { rows = metadata ? archivedRows((await client.get(metadata, { signal })).bytes) : []; } catch (error) {
        if (cut(error)) throw error;
        problems.push(`No ride is marked as taken on the device: ${text(error)}`);
        return 0;
    }
    const stored = new Map((await library.rides()).flatMap(ride => ride.device ? [[ride.device, ride.key] as const] : []));
    let marked = 0;
    for (const entry of rides) {
        const key = stored.get(`${catalog.storeId}:${entry.objectId}`);
        if (!key || rows.some(row => row.objectId === entry.objectId && row.revision === entry.revision
            && row.payloadLength === entry.payloadLength && row.payloadCrc32 === entry.payloadCrc32)) continue;
        const file = await library.rideFile(key);
        if (!file || BigInt(file.size) !== entry.payloadLength || Crc32.of(new Uint8Array(await file.arrayBuffer())) !== entry.payloadCrc32) continue;
        try {
            await client.archiveRide({ storeId: catalog.storeId, ...entry }, signal);
            marked++;
        } catch (error) {
            if (cut(error)) throw error;
            if (refused(error, 'invalid-request')) continue;
            if (!refused(error, 'unsupported', 'no-space')) problems.push(text(error));
            break;
        }
    }
    return marked;
}

/** Remembers the map, the number of routes and the bytes by kind of `catalog`. */
function rememberContents(catalog: Catalog): void {
    const bytes: DeviceBytes = { map: 0, routes: 0, rides: 0, other: 0 };
    for (const entry of catalog.entries) {
        const kind = entry.kind === ObjectKind.MapShard ? 'map' : entry.kind === ObjectKind.Route ? 'routes' : entry.kind === ObjectKind.Ride ? 'rides' : 'other';
        bytes[kind] += Number(entry.payloadLength);
    }
    const map = ofKind(catalog, ObjectKind.MapShard).sort(byId)[0];
    remember({ at: Date.now(), map: map?.displayName ?? null, routes: ofKind(catalog, ObjectKind.Route).length, bytes });
}
