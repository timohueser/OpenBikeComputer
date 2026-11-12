/** Saves the library and the settings as one backup file, and adds the records of a backup file to the library. */
import { isRideRecord, isRouteRecord, stateChecks, type LibraryState, type RideRecord } from './records';
import type { Contents, Library } from './store';
import { unzip, zip } from './zip';

const format = 'openbikecomputer-library', version = 1;

const filePath = (ride: RideRecord) => `rides/${ride.key}.${ride.format}`;

/** A ZIP file of `library.json` and the source file of each ride (`specs/library-backup.md`). */
export async function saveBackup(library: Library, settings: object): Promise<Blob> {
    const { routes, versions, rides, files, state } = await library.contents();
    const kept = rides.filter(ride => files.has(ride.key));
    const json = JSON.stringify({ format, version, routes, versions: Object.fromEntries(routes.flatMap(r => versions[r.id] ? [[r.id, versions[r.id]]] : [])), rides: kept, state, settings });
    return zip([{ name: 'library.json', data: new Blob([json], { type: 'application/json' }) }, ...kept.map(ride => ({ name: filePath(ride), data: files.get(ride.key)! }))]);
}

const isObject = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The contents and the settings of a backup file. It refuses a file of another version and a damaged file. */
async function readBackup(file: Blob): Promise<{ contents: Contents; settings: Record<string, unknown> }> {
    const notBackup = new Error('This file is not a library backup.');
    const entries = await unzip(file).catch(() => { throw notBackup; });
    const text = await entries.get('library.json')?.text();
    let json: unknown;
    try { json = JSON.parse(text ?? ''); } catch { throw notBackup; }
    if (!isObject(json) || json.format !== format) throw notBackup;
    if (json.version !== version) throw new Error('This backup is from another version of the app. Update the app and try again.');
    const { routes, versions, rides, state, settings } = json;
    const damaged = new Error('This backup is damaged. Nothing was loaded.');
    if (!Array.isArray(routes) || !routes.every(isRouteRecord) || new Set(routes.map(r => r.id)).size !== routes.length) throw damaged;
    if (!Array.isArray(rides) || !rides.every(isRideRecord) || new Set(rides.map(r => r.key)).size !== rides.length) throw damaged;
    const ids = new Set(routes.map(r => r.id));
    if (!isObject(versions) || !Object.entries(versions).every(([id, list]) => ids.has(id) && Array.isArray(list))) throw damaged;
    if (!isObject(state) || !Object.entries(state).every(([key, value]) => key in stateChecks && stateChecks[key as keyof LibraryState](value))) throw damaged;
    if (!isObject(settings)) throw damaged;
    const files = new Map<string, Blob>();
    for (const ride of rides) {
        const data = entries.get(filePath(ride));
        if (!data) throw damaged;
        files.set(ride.key, data);
    }
    return { contents: { routes, versions: versions as Record<string, unknown[]>, rides, files, state: state as Partial<LibraryState> }, settings };
}

/** Adds the new records of a backup file, all or none. A route is new when its id is not stored; a ride when its key is
 * neither stored nor deleted. The settings come back only when the library was empty. */
export async function loadBackup(library: Library, file: Blob): Promise<{ routes: number; rides: number; settings?: Record<string, unknown> }> {
    const { contents, settings } = await readBackup(file);
    const { routes, rides, empty } = await library.merge(contents);
    return { routes, rides, ...empty && { settings } };
}
