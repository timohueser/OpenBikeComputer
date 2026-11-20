/** One sync with the remembered device: the new rides come into the library and are marked as taken, the deleted routes
 * leave the device, and the routes that wait go to it. And one send of the map that waits. */
import type { JobContext } from '../device/progress';
import { displayName } from '../device/write';
import type { NextMap, SentRoute } from '../library/records';
import type { Library } from '../library/store';
import { sentKey } from '../library/summary';
import { partDays, planTitle } from '../planner/editor';
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
import { partName, startDay, tripKey, tripObject } from './trip';

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
    /** Routes removed that went as one route. */
    removed: number;
    /** Routes removed that went as a trip. */
    removedTrips: number;
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
    const report: SyncReport = { rides: 0, marked: 0, sent: 0, trips: 0, removed: 0, removedTrips: 0, waypointsLeft: 0, tried: [], problem: null };
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
            const { copy } = await entryOn(onCard, id), trip = copy.trips.some(Boolean) || copy.days.length > 1;
            const had = [...copy.trips, ...copy.days].some(object => onCard.objects.has(object));
            if (!await shrink(onCard, id, copy, 0, 0, '')) continue;
            await library.changeState('sent', routes => Object.fromEntries(Object.entries(routes ?? {}).filter(([other]) => other !== id)));
            await library.changeState('leaving', ids => (ids ?? []).filter(other => other !== id));
            if (had) report[trip ? 'removedTrips' : 'removed']++;
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
        // A day route that the device lost, or that the rider deleted on it, is gone: the entry keeps its id, and the route
        // stays on the device. A gone trip object empties its slot, and while a day of its part is on the card also the key,
        // so the next send makes it again. With no object left, the entry goes.
        const held = new Set([...ofKind(last, ObjectKind.Route), ...ofKind(last, ObjectKind.Trip)].map(key));
        const now = (copy: SentRoute): SentRoute | null => {
            if (copy.card !== last.storeId) return copy;
            const lost = copy.trips.some((trip, p) => trip && !held.has(trip) && copy.days.slice(p * partDays, (p + 1) * partDays).some(day => held.has(day)));
            const left = { ...copy, key: lost ? '' : copy.key, gone: copy.days.filter(day => day && !held.has(day)), trips: copy.trips.map(trip => held.has(trip) ? trip : '') };
            return [...left.trips, ...left.days].some(object => held.has(object)) ? left : null;
        };
        if (Object.values(await library.state('sent') ?? {}).some(copy => JSON.stringify(now(copy)) !== JSON.stringify(copy))) {
            await library.changeState('sent', routes => Object.fromEntries(Object.entries(routes ?? {}).flatMap(([id, copy]) => {
                const left = now(copy);
                return left ? [[id, left]] : [];
            })));
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
    /** The trip objects of the card at the start of the sync, with their trip keys and day route ids, read on first use. */
    trips?: Promise<Map<string, { key: string; days: string[] }>>;
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
const save = (library: Library, id: string, copy: SentRoute) => library.changeState('sent', routes => ({ ...routes, [id]: structuredClone(copy) }));
/** The parts of a plan: days 1 to 32 and days 33 to 64 (`partDays`). */
const PARTS = [0, 1];

/** The trip objects of the card with their trip keys in decimal and their day route ids, read once in a sync. */
function tripsOf(card: Card): Promise<Map<string, { key: string; days: string[] }>> {
    return card.trips ??= (async () => {
        const trips = new Map<string, { key: string; days: string[] }>();
        for (const entry of card.objects.values()) {
            if (entry.kind !== ObjectKind.Trip || entry.payloadLength < 64n) continue;
            let bytes;
            try { bytes = (await card.client.get(entry, { signal: card.signal })).bytes; } catch (error) {
                if (cut(error)) throw error;
                continue;
            }
            const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength), count = Math.min(view.getUint16(2, true), Math.floor((bytes.length - 64) / 16));
            trips.set(key(entry), { key: `${view.getBigUint64(56, true)}`, days: Array.from({ length: count }, (_, i) => `${view.getBigUint64(64 + 16 * i, true)}`) });
        }
        return trips;
    })();
}

/**
 * The entry of route `id` on the card, with no key while the steps change it, and the objects that it may not take: those
 * of the other entries on the card, and the routes that a foreign trip object lists. A day route that a foreign trip object
 * lists is not this library's to change, and a trip object that the card does not hold is gone: both read as no object. A
 * day route that is gone keeps its id. An entry of another card counts as no entry. A trip object is this library's when an
 * entry on the card names it, or when its trip key is one that an entry stores (on any card) or one of `current`, the keys
 * of the parts of the route now; else it is foreign, such as a trip of the phone, which takes routes into its trips by their
 * content. A part with no trip object takes the trip object with its key in `current` or with the key that the entry
 * stored on this card, so neither a lost answer nor a send to another card in between leaves a second trip.
 */
async function entryOn(card: Card, id: string, current: string[] = []): Promise<{ copy: SentRoute; taken: Set<string> }> {
    const sent = await card.library.state('sent') ?? {}, stored = sent[id]?.card === card.id ? sent[id] : undefined, trips = await tripsOf(card);
    const ids = new Set(Object.values(sent).flatMap(them => them.card === card.id ? them.trips : []));
    const keys = new Set([...Object.values(sent).flatMap(them => them.tripKeys), ...current]);
    const foreign = new Set([...trips].flatMap(([trip, { key, days }]) => ids.has(trip) || keys.has(key) ? [] : days));
    const copy: SentRoute = { key: '', card: card.id, days: (stored?.days ?? []).map(day => foreign.has(day) ? '' : day), crcs: [...stored?.crcs ?? []],
        gone: [...stored?.gone ?? []], trips: PARTS.map(p => card.objects.has(stored?.trips[p] ?? '') ? stored!.trips[p] : ''),
        tripKeys: PARTS.map(p => stored?.tripKeys[p] ?? '') };
    const taken = new Set([...Object.entries(sent).flatMap(([other, them]) => other === id || them.card !== card.id ? [] : [...them.trips, ...them.days]), ...foreign]);
    for (const p of PARTS) {
        const wanted = [current[p], copy.tripKeys[p]];
        copy.trips[p] ||= [...trips].find(([trip, { key }]) => wanted.includes(key) && card.objects.has(trip) && !taken.has(trip))?.[0] ?? '';
    }
    return { copy, taken };
}

/** Removes the trip objects of the parts from `parts` on, then the day routes past `days`, the last first, and writes the
 * entry after each removal. A gone object counts as removed. False when the device refused a removal: `problems` gets its
 * sentence after `prefix`. */
async function shrink(card: Card, id: string, copy: SentRoute, parts: number, days: number, prefix: string): Promise<boolean> {
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
    for (let p = parts; p < copy.trips.length; p++) {
        const trip = copy.trips[p];
        [copy.trips[p], copy.tripKeys[p]] = ['', ''];
        if (!await remove(trip)) return false;
    }
    while (copy.days.length > days) {
        copy.crcs.pop();
        if (!await remove(copy.days.pop()!)) return false;
    }
    return true;
}

/**
 * Sends the route `id` that waits: a plan of one riding day as one route, else one route for each riding day and a trip
 * object for each part of 32 days, part 1 first. An object whose copy on the card has the same length and CRC is skipped,
 * another object of the entry is replaced in place, and a missing one is created. A route on the card with the length,
 * the CRC and the name of a missing day takes its place, so the card gets no second copy. A day that is gone from the
 * card, and whose file has the CRC that the entry stores, is left out: the rider deleted it on the device, and the trip
 * object lists its old id. When the menu of the device has no room for the new objects of a part, the plan stops there.
 * Gives how the route went, or nothing while it waits: then `problems` says why.
 */
async function sendRoute(card: Card, id: string, legs: LegCache): Promise<{ trip: boolean; left: number } | undefined> {
    const { client, library, signal, problems } = card;
    const route = await library.item(id);
    if (!route || !('id' in route) || !isPlan(route)) { problems.push('A route that waits cannot be read.'); return; }
    const name = displayName(route.name || planTitle(route.trip), 'Route'), plan = `“${name}” waits: `;
    let waits = plan, routes: DeviceRoute[];
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
    const parts = days > 1 ? Math.ceil(days / partDays) : 0, keys = PARTS.map(p => tripKey(id, route.trip, p + 1));
    const { copy, taken } = await entryOn(card, id, keys.map(String));

    // A lost answer of an earlier put left a copy on the card. An object of another entry on this card is not one.
    const unnamed = (entry: CatalogEntry) => !taken.has(key(entry)) && !copy.trips.includes(key(entry)) && !copy.days.includes(key(entry));
    files.forEach((file, i) => {
        copy.crcs[i] ??= 0;
        const object = copy.days[i] ?? '';
        if (card.objects.has(object) || object && copy.crcs[i] === file.crc) return;
        const copied = [...card.objects.values()].find(entry => entry.kind === file.kind && entry.displayName === file.name && holds(entry, file) && unnamed(entry));
        copy.days[i] = copied ? key(copied) : '';
    });
    const leftOut = (i: number) => !!copy.days[i] && !card.objects.has(copy.days[i]);
    // The trip objects of the parts that the plan no longer has go first. A route of one day also leaves its other days.
    if (!await shrink(card, id, copy, parts, days > 1 ? copy.days.length : 1, waits)) return;

    /** Puts `file` on `object`, or creates it, unless `object` holds it, and writes the entry. False when the route stops: the
     * device refused the put, or the rider deleted the route during its upload. */
    const put = async (file: Payload, object: string, set: (object: string) => void) => {
        const target = card.objects.get(object);
        if (target && holds(target, file)) {
            set(object);
            return true;
        }
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
    for (let p = 0; p * partDays < days; p++) {
        const range = files.slice(p * partDays, (p + 1) * partDays).map((_, k) => p * partDays + k);
        waits = p ? `Part ${p + 1} of “${name}” waits: ` : plan;
        // A part with all its days left out and no trip object on the card is not on the device any more.
        if (range.every(leftOut) && !copy.trips[p]) continue;
        // The menu of the device shows only the newest 64 routes: the days that the plan no longer has make room first.
        const needed = range.filter(i => !copy.days[i]).length;
        if (needed > MENU.routes - card.count.routes && !await shrink(card, id, copy, parts, days, waits)) return;
        const free = Math.max(0, MENU.routes - card.count.routes);
        if (needed > free) {
            problems.push(`${waits}the bike computer has room for ${free} more ${free === 1 ? 'route' : 'routes'}, it needs ${needed}. Remove ridden or unused routes on the bike computer.`);
            return;
        }
        if (parts && !copy.trips[p] && card.count.trips >= MENU.trips) {
            problems.push(`${waits}the bike computer holds ${MENU.trips} trips, as many as it can. Remove a ridden or unused trip on the bike computer.`);
            return;
        }
        for (const i of range) {
            if (!leftOut(i) && !await put(files[i], copy.days[i], object => { [copy.days[i], copy.crcs[i]] = [object, files[i].crc]; })) return;
        }
        if (!parts) continue;
        // A lost answer of this put leaves the trip key in the entry, so the next sync finds the trip object.
        if (copy.tripKeys[p] !== `${keys[p]}`) {
            copy.tripKeys[p] = `${keys[p]}`;
            await save(library, id, copy);
        }
        const title = partName(name, p + 1, parts);
        const trip = tripObject(keys[p], title, startDay(route.trip, p + 1), range.map(i => ({ route: BigInt(copy.days[i]), joinM: 0, leaveM: 0xffff_ffff })));
        if (!await put(payload(ObjectKind.Trip, title, trip), copy.trips[p], object => { copy.trips[p] = object; })) return;
    }
    // The trip objects name only the kept days now, so a refused removal leaves no removed day in them.
    if (!await shrink(card, id, copy, parts, days, plan)) return;
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
