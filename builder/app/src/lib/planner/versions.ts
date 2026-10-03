import { cumulative, itineraryDays, routeCoordinates, storedPlan, type Trip } from './editor';
import { isTrip, storedTrip } from './trip-validation';

export interface Version {
    id: string;
    at: string;
    name?: string;
    summary: string;
    trip: Trip;
}

type VersionStore = Pick<Storage, 'getItem' | 'setItem'>;

const key = 'obc-planner-lab-versions-v1';
const unnamedKept = 10;

export function versionSummary(trip: Trip): string {
    const endpoints = trip.points.filter(p => p.kind === 'start' || p.kind === 'finish');
    if (endpoints.length < 2) return endpoints.length ? `${endpoints[0].kind === 'start' ? 'Start' : 'Finish'} chosen` : 'Empty plan';
    const distance = `${cumulative(routeCoordinates(trip)).at(-1)!.toFixed(1)} km`;
    if (trip.mode === 'route') return `Single route · ${distance}`;
    const days = itineraryDays(trip).length;
    const pinned = trip.points.filter(p => p.kind === 'night').length;
    const parts = [`${days} ${days === 1 ? 'day' : 'days'}`];
    if (pinned) parts.push(`${pinned} ${pinned === 1 ? 'night' : 'nights'} pinned`);
    return [...parts, distance].join(' · ');
}

/** Newest first. */
export function listVersions(store?: VersionStore): Version[] {
    try {
        const list = JSON.parse((store ?? localStorage).getItem(key) ?? '[]');
        const ids = new Set<string>();
        return Array.isArray(list) ? list.filter((value): value is Version => {
            if (!isVersion(value) || ids.has(value.id)) return false;
            ids.add(value.id);
            return true;
        }) : [];
    } catch {
        return [];
    }
}

function isVersion(value: unknown): value is Version {
    if (!value || typeof value !== 'object') return false;
    const version = value as Partial<Version>;
    return typeof version.id === 'string' && version.id.length > 0
        && typeof version.at === 'string' && Number.isFinite(Date.parse(version.at))
        && typeof version.summary === 'string' && (version.name === undefined || typeof version.name === 'string')
        && isTrip(version.trip);
}

/** Named versions stay until deleted; only the newest unnamed ones are kept. */
export function saveVersion(trip: Trip, name?: string, store: VersionStore = localStorage): Version {
    const version: Version = { id: crypto.randomUUID(), at: new Date().toISOString(), summary: versionSummary(trip), trip: storedPlan(trip) };
    if (name?.trim()) version.name = name.trim();
    let unnamed = 0;
    const kept = [version, ...listVersions(store)].filter(v => v.name || ++unnamed <= unnamedKept);
    store.setItem(key, JSON.stringify(kept));
    return version;
}

/** Names the newest version; an empty name leaves it unnamed. */
export function renameLatest(name: string, store: VersionStore = localStorage): void {
    const [latest, ...rest] = listVersions(store);
    if (!latest) return;
    const { name: _, ...unnamed } = latest;
    store.setItem(key, JSON.stringify([name.trim() ? { ...unnamed, name: name.trim() } : unnamed, ...rest]));
}

export function deleteVersion(id: string, store: VersionStore = localStorage): void {
    store.setItem(key, JSON.stringify(listVersions(store).filter(v => v.id !== id)));
}

export function readVersion(id: string, store: VersionStore = localStorage): Trip | undefined {
    return storedTrip(listVersions(store).find(v => v.id === id)?.trip);
}
