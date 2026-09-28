import { cumulative, itineraryDays, routeCoordinates, type Trip } from './editor';

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
    const distance = `${cumulative(routeCoordinates(trip)).at(-1)!.toFixed(1)} km`;
    if (trip.mode === 'route') return `Single route · ${distance}`;
    const days = itineraryDays(trip).length;
    const pinned = trip.points.filter(p => p.kind === 'night').length;
    const parts = [`${days} ${days === 1 ? 'day' : 'days'}`];
    if (pinned) parts.push(`${pinned} ${pinned === 1 ? 'night' : 'nights'} pinned`);
    return [...parts, distance].join(' · ');
}

/** Newest first. */
export function listVersions(store: VersionStore = localStorage): Version[] {
    try {
        const list = JSON.parse(store.getItem(key) ?? '[]');
        return Array.isArray(list) ? list : [];
    } catch {
        return [];
    }
}

/** Named versions stay until deleted; only the newest unnamed ones are kept. */
export function saveVersion(trip: Trip, name?: string, store: VersionStore = localStorage): Version {
    const version: Version = { id: crypto.randomUUID(), at: new Date().toISOString(), summary: versionSummary(trip), trip };
    if (name?.trim()) version.name = name.trim();
    let unnamed = 0;
    const kept = [version, ...listVersions(store)].filter(v => v.name || ++unnamed <= unnamedKept);
    store.setItem(key, JSON.stringify(kept));
    return version;
}

export function deleteVersion(id: string, store: VersionStore = localStorage): void {
    store.setItem(key, JSON.stringify(listVersions(store).filter(v => v.id !== id)));
}

export function readVersion(id: string, store: VersionStore = localStorage): Trip | undefined {
    return listVersions(store).find(v => v.id === id)?.trip;
}
