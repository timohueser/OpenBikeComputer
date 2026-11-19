/** One sync with the remembered device: the new rides come into the library and are marked as taken, the deleted routes
 * leave the device, and the routes that wait go to it. And one send of the map that waits. */
import type { JobContext } from '../device/progress';
import { displayName } from '../device/write';
import type { NextMap, SentRoute } from '../library/records';
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
import { EntryFlags, ObjectKind, type CatalogEntry, type PutResponse } from '../usb/protocol';
import type { MapArea } from './area';
import { archivedRows } from './metadata';
import { remember, rememberCard, remembered, type DeviceBytes } from './remembered.svelte';
import { rideRecord } from './ride';
import { dayRoutes, type DeviceRoute } from './route';
import { startDay, tripKey, tripObject } from './trip';

/** What a sync uses of the client. */
export type SyncClient = Pick<FlatStoreClient, 'list' | 'get' | 'put' | 'remove' | 'archiveRide'>;

export interface SyncReport {
    /** Rides that came into the library. */
    rides: number;
    /** Rides that the device now marks as taken. */
    marked: number;
    /** Routes sent as one route. */
    sent: number;
    /** Routes sent as day routes and a trip object, each once, however many of its objects went. */
    trips: number;
    removed: number;
    /** Waypoints of the sent routes past the 32 that the device reads. */
    waypointsLeft: number;
    /** The route ids that the sync tried to send or to remove. */
    tried: string[];
    /** What did not finish, in sentences for the rider; null when all finished. */
    problem: string | null;
}

export const CABLE_OUT = 'The sync stopped when the cable came out. It goes on at the next connection.';
export const NO_ROOM = 'The memory card has no room for the new map. The old map stays until the new one is complete, so the card needs room for both.';
export const MAP_LOST = 'The new map was not sent. “Send the map” starts again.';

/** Builds the map `next` and puts it on the device; `area` is what it sent. The one of the app is `buildMap` of `map.svelte.ts`. */
export type BuildMap = (next: NextMap, ctx: JobContext) => Promise<Pick<PutResponse, 'objectId' | 'revision'> & { area: MapArea }>;

const unused = (entry: CatalogEntry) => (entry.flags & (EntryFlags.Recording | EntryFlags.Retained)) === 0;
const ofKind = (catalog: Catalog, kind: number) => catalog.entries.filter(entry => entry.kind === kind && unused(entry));
const key = (entry: CatalogEntry) => entry.objectId.toString();
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
 * write of another tab or page is not lost. A ride is stored only after its whole `get`, and each object of a route is in
 * `sent` right after its `put`, so a cut cable loses nothing. A lost `put` answer, and then a new line from the routing,
 * can leave a second copy of a route on the device. */
export async function syncDevice(client: SyncClient, serial: string, library: Library, signal: AbortSignal): Promise<SyncReport> {
    const report: SyncReport = { rides: 0, marked: 0, sent: 0, trips: 0, removed: 0, waypointsLeft: 0, tried: [], problem: null };
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
        const routes = ofKind(catalog, ObjectKind.Route), trips = ofKind(catalog, ObjectKind.Trip);
        const onCard: Card = { client, library, signal, problems, id: card, objects: new Map([...routes, ...trips].map(entry => [key(entry), entry])),
            count: { routes: routes.length, trips: trips.length } };
        for (const id of await library.state('leaving') ?? []) {
            report.tried.push(id);
            const copy = await entryOn(onCard, id), had = [copy.trip, ...copy.days].some(Boolean);
            if (!await shrink(onCard, id, copy, 0, false, '')) continue;
            await library.changeState('sent', routes => Object.fromEntries(Object.entries(routes ?? {}).filter(([other]) => other !== id)));
            await library.changeState('leaving', ids => (ids ?? []).filter(other => other !== id));
            if (had) report.removed++;
        }

        const legs = new LegCache();
        for (const id of await library.state('waiting') ?? []) {
            report.tried.push(id);
            const sent = await sendRoute(onCard, id, legs);
            if (!sent) continue;
            report[sent.trip ? 'trips' : 'sent']++;
            report.waypointsLeft += sent.left;
        }

        const last = await client.list({ signal });
        // A route of which the device lost an object, or the rider deleted one on it, can be sent again.
        const held = new Set([...ofKind(last, ObjectKind.Route), ...ofKind(last, ObjectKind.Trip)].map(key));
        const lost = (copy: SentRoute) => copy.card === last.storeId && [copy.trip, ...copy.days].some(object => object && !held.has(object));
        if (Object.values(await library.state('sent') ?? {}).some(lost)) {
            await library.changeState('sent', routes => Object.fromEntries(Object.entries(routes ?? {}).filter(([, copy]) => !lost(copy))));
        }
        rememberContents(last);
    } catch (error) {
        problems.push(cut(error) || signal.aborted ? CABLE_OUT : text(error));
    }
    report.problem = problems.join(' ') || null;
    return report;
}

/** The routes and the trips that the menu of the device holds (`MAX_ROUTES` and `MAX_TRIPS` of `obc-app`). */
const MENU = { routes: 64, trips: 16 };

/** What the route steps of one sync share: the card `id` with its route and trip objects by id, the number of its routes and
 * trips, which the steps keep current, and the sentences of what did not finish. */
interface Card {
    client: SyncClient;
    library: Library;
    signal: AbortSignal;
    problems: string[];
    id: string;
    objects: Map<string, CatalogEntry>;
    count: { routes: number; trips: number };
}

/** An object that a route needs on the card. */
interface Payload {
    kind: ObjectKind;
    name: string;
    bytes: Uint8Array;
    crc: number;
}

const payload = (kind: ObjectKind, name: string, bytes: Uint8Array): Payload => ({ kind, name, bytes, crc: Crc32.of(bytes) });
const holds = (entry: CatalogEntry, file: Payload) => entry.payloadLength === BigInt(file.bytes.length) && entry.payloadCrc32 === file.crc;
const save = (library: Library, id: string, copy: SentRoute) => library.changeState('sent', routes => ({ ...routes, [id]: { ...copy, days: [...copy.days] } }));

/** The entry of route `id` on the card, with no key while the steps change it. An object that the card does not hold is
 * gone: it reads as no object. An entry of another card counts as no entry. */
async function entryOn(card: Card, id: string): Promise<SentRoute> {
    const copy = (await card.library.state('sent'))?.[id], held = (object: string | null) => object && card.objects.has(object) ? object : '';
    return copy?.card === card.id ? { key: '', card: card.id, days: copy.days.map(held), trip: held(copy.trip) || null }
        : { key: '', card: card.id, days: [], trip: null };
}

/** Removes the trip object of `copy` unless `keepTrip`, then its day routes past `days`, the last first, and writes the entry
 * after each removal. "Not found" counts as removed. False when the device refused a removal: `problems` gets its sentence
 * after `prefix`. */
async function shrink(card: Card, id: string, copy: SentRoute, days: number, keepTrip: boolean, prefix: string): Promise<boolean> {
    const remove = async (object: string) => {
        const entry = card.objects.get(object);
        if (!entry) return true;
        try { await card.client.remove(entry, card.signal); } catch (error) {
            if (cut(error)) throw error;
            if (!refused(error, 'not-found')) { card.problems.push(prefix + text(error)); return false; }
        }
        card.objects.delete(object);
        card.count[entry.kind === ObjectKind.Trip ? 'trips' : 'routes']--;
        await save(card.library, id, copy);
        return true;
    };
    if (!keepTrip && copy.trip) {
        const trip = copy.trip;
        copy.trip = null;
        if (!await remove(trip)) return false;
    }
    while (copy.days.length > days) {
        if (!await remove(copy.days.pop()!)) return false;
    }
    return true;
}

/** The trip object `name` with the trip key `wanted` on the card, of those that `open` lets through. */
async function tripOn(card: Card, name: string, wanted: bigint, open: (entry: CatalogEntry) => boolean): Promise<string | undefined> {
    for (const entry of card.objects.values()) {
        if (entry.kind !== ObjectKind.Trip || entry.displayName !== name || entry.payloadLength < 64n || !open(entry)) continue;
        let bytes;
        try { bytes = (await card.client.get(entry, { signal: card.signal })).bytes; } catch (error) {
            if (cut(error)) throw error;
            continue;
        }
        if (new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getBigUint64(56, true) === wanted) return key(entry);
    }
}

/**
 * Sends the route `id` that waits: a plan of one riding day as one route, else one route for each riding day and then the
 * trip object. An object whose copy on the card has the same length and CRC is skipped, another object of the entry is
 * replaced in place, and a missing one is created. A route on the card with the length, the CRC and the name of a missing
 * day takes its place, and so does a trip object with the name and the trip key of the route, so the card gets no second
 * copy. When the menu of the device has no room for the new objects, nothing goes. Gives how the route went, or nothing
 * while it waits: then `problems` says why.
 */
async function sendRoute(card: Card, id: string, legs: LegCache): Promise<{ trip: boolean; left: number } | undefined> {
    const { client, library, signal, problems } = card;
    const route = await library.item(id);
    if (!route || !('id' in route) || !isPlan(route)) { problems.push('A route that waits cannot be read.'); return; }
    const name = displayName(route.name || planTitle(route.trip), 'Route'), waits = `“${name}” waits: `;
    let routes: DeviceRoute[];
    try {
        const line = await calculateLine(route.trip, signal, legs), gaps = line.gapLegs.length;
        // A line with a gap is not complete; a point that the line passes by is not on it.
        if (gaps) {
            problems.push(`${waits}${gaps} ${gaps > 1 ? 'legs have' : 'leg has'} no legal path. Change it in the planner.`);
            return;
        }
        routes = await dayRoutes(route, line);
    } catch (error) {
        if (signal.aborted) throw error;
        problems.push(waits + text(error));
        return;
    }
    const files = routes.map(day => payload(ObjectKind.Route, displayName(day.name, 'Route'), day.bytes)), days = files.length;
    const copy = await entryOn(card, id), sent = await library.state('sent') ?? {}, tripId = tripKey(id, route.trip);

    // A lost answer of an earlier put, or an entry that a sync dropped, left objects of the route on the card. An object of
    // another entry on this card is not one.
    const taken = new Set(Object.entries(sent).flatMap(([other, them]) => other === id || them.card !== card.id ? [] : [them.trip, ...them.days]));
    const unnamed = (entry: CatalogEntry) => !taken.has(key(entry)) && copy.trip !== key(entry) && !copy.days.includes(key(entry));
    files.forEach((file, i) => {
        const copied = [...card.objects.values()].find(entry => entry.kind === file.kind && entry.displayName === file.name && holds(entry, file) && unnamed(entry));
        copy.days[i] ||= copied ? key(copied) : '';
    });
    // The days of a trip object change when a day route is made again, its key does not.
    if (days > 1 && !copy.trip) copy.trip = await tripOn(card, name, tripId, unnamed) ?? null;

    if (!await shrink(card, id, copy, days, days > 1, waits)) return;

    const needed = copy.days.filter(object => !object).length, free = Math.max(0, MENU.routes - card.count.routes);
    if (needed > free) {
        problems.push(`${waits}the bike computer has room for ${free} more ${free === 1 ? 'route' : 'routes'}, it needs ${needed}. Remove ridden or unused routes on the bike computer.`);
        return;
    }
    if (days > 1 && !copy.trip && card.count.trips >= MENU.trips) {
        problems.push(`${waits}the bike computer holds ${MENU.trips} trips, as many as it can. Remove a ridden or unused trip on the bike computer.`);
        return;
    }

    /** Puts `file` on `object`, or creates it, unless `object` holds it, and writes the entry. False when the route stops: the
     * device refused the put, or the rider deleted the route during its upload. */
    const put = async (file: Payload, object: string | null, set: (object: string) => void) => {
        const target = object ? card.objects.get(object) : undefined;
        if (target && holds(target, file)) return true;
        const send = (on?: CatalogEntry) =>
            client.put({ objectId: on?.objectId, expectedRevision: on?.revision, kind: file.kind, displayName: file.name }, file.bytes, { signal });
        let answer;
        try {
            answer = await send(target).catch(error => target && refused(error, 'not-found') ? send() : Promise.reject(error));
        } catch (error) {
            if (cut(error)) throw error;
            problems.push(waits + text(error));
            return false;
        }
        if (answer.objectId !== target?.objectId) card.count[file.kind === ObjectKind.Trip ? 'trips' : 'routes']++;
        set(answer.objectId.toString());
        await save(library, id, copy);
        // The rider deleted the route during its upload, after `removeRoute` looked at `sent`.
        if (await library.item(id)) return true;
        await library.changeState('leaving', ids => [...(ids ?? []).filter(other => other !== id), id]);
        return false;
    };
    for (const [i, file] of files.entries()) if (!await put(file, copy.days[i], object => { copy.days[i] = object; })) return;
    if (days > 1) {
        const trip = tripObject(tripId, name, startDay(route.trip), copy.days.map(object => ({ route: BigInt(object), joinM: 0, leaveM: 0xffff_ffff })));
        if (!await put(payload(ObjectKind.Trip, name, trip), copy.trip, object => { copy.trip = object; })) return;
    }
    await save(library, id, { ...copy, key: sentKey(route) });
    await library.changeState('waiting', ids => (ids ?? []).filter(other => other !== id));
    return { trip: days > 1, left: routes.reduce((sum, day) => sum + day.left, 0) };
}

/**
 * Sends the map that waits to the device `serial`, only when it is the remembered device. After the put it remembers the
 * area and the colours of the map object, and the map waits no more. A map that the rider chose during the build still
 * waits. Returns the sentence for the rider when the map did not go, else null. A cut cable and a stop keep the old map
 * on the card.
 */
export async function sendMap(serial: string, library: Library, build: BuildMap, ctx: JobContext): Promise<string | null> {
    const next = await library.state('nextMap');
    if (!next || remembered.device?.serial !== serial) return null;
    let object: string, area: MapArea;
    try {
        const put = await build(next, ctx);
        [object, area] = [`${put.objectId}:${put.revision}`, put.area];
    } catch (error) {
        if (refused(error, 'no-space')) return NO_ROOM;
        return cut(error) || ctx.signal.aborted ? MAP_LOST : text(error);
    }
    // The map is on the device: a failed write of the library is no failed send.
    const sent = JSON.stringify(next);
    remember({ mapObject: object, built: { object, ...area } });
    await library.changeState('nextMap', now => now && JSON.stringify(now) !== sent ? now : null);
    return null;
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
    remember({ at: Date.now(), map: map?.displayName ?? null, mapObject: map ? `${map.objectId}:${map.revision}` : null, routes: ofKind(catalog, ObjectKind.Route).length, bytes });
}
