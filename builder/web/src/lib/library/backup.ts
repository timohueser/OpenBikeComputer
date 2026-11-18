/** Saves the library and the settings as one backup file, and adds the records of a backup file to the library. */
import { smallPicture } from './photos';
import { isPhotoEntry, isRideRecord, isRouteRecord, stateChecks, type LibraryState, type PhotoEntry, type RideRecord } from './records';
import type { Contents, Library } from './store';
import { unzip, zip, type Entry } from './zip';

const format = 'openbikecomputer-library', version = 1;

const filePath = (ride: RideRecord) => `rides/${ride.key}.${ride.format}`;
const photoPath = (photo: PhotoEntry) => `photos/${photo.id}.jpg`;

/** A ZIP file of `library.json`, the source file of each ride and, with `photos`, the copy of each photo
 * (`specs/library-backup.md`). It reads one photo record at a time; the file holds the copies, not their bytes in
 * memory. It throws `TooLarge` of `zip.ts` before it reads a copy. */
export async function saveBackup(library: Library, settings: object, photos = true): Promise<Blob> {
    const { routes, versions, rides, files, state } = await library.contents();
    const kept = rides.filter(ride => files.has(ride.key)), keys = new Set(kept.map(ride => ride.key));
    const records: PhotoEntry[] = [], copies: Entry[] = [];
    for (const id of photos ? await library.photoIds() : []) {
        const photo = await library.photo(id);
        if (!photo || !keys.has(photo.ride)) continue;
        const { image, small: _, ...record } = photo;
        records.push(record);
        copies.push({ name: photoPath(record), data: image });
    }
    const json = JSON.stringify({ format, version, routes, versions: Object.fromEntries(routes.flatMap(r => versions[r.id] ? [[r.id, versions[r.id]]] : [])), rides: kept, state, settings,
        ...photos && { photos: records } });
    return zip([{ name: 'library.json', data: new Blob([json], { type: 'application/json' }) }, ...kept.map(ride => ({ name: filePath(ride), data: files.get(ride.key)! })), ...copies]);
}

const isObject = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The contents, the photos and the settings of a backup file. It refuses a file of another version and a damaged file. */
async function readBackup(file: Blob): Promise<{ contents: Contents; photos: { record: PhotoEntry; image: Blob }[]; settings: Record<string, unknown> }> {
    const notBackup = new Error('This file is not a library backup.');
    const entries = await unzip(file).catch(() => { throw notBackup; });
    const text = await entries.get('library.json')?.text();
    let json: unknown;
    try { json = JSON.parse(text ?? ''); } catch { throw notBackup; }
    if (!isObject(json) || json.format !== format) throw notBackup;
    if (json.version !== version) throw new Error('This backup is from another version of the app. Update the app and try again.');
    const { routes, versions, rides, state, settings, photos = [] } = json;
    const damaged = new Error('This backup is damaged. Nothing was loaded.');
    if (!Array.isArray(routes) || !routes.every(isRouteRecord) || new Set(routes.map(r => r.id)).size !== routes.length) throw damaged;
    if (!Array.isArray(rides) || !rides.every(isRideRecord) || new Set(rides.map(r => r.key)).size !== rides.length) throw damaged;
    const ids = new Set(routes.map(r => r.id));
    if (!isObject(versions) || !Object.entries(versions).every(([id, list]) => ids.has(id) && Array.isArray(list))) throw damaged;
    if (!isObject(state) || !Object.entries(state).every(([key, value]) => Object.hasOwn(stateChecks, key) && stateChecks[key as keyof LibraryState](value))) throw damaged;
    if (!isObject(settings)) throw damaged;
    if (!Array.isArray(photos) || !photos.every(isPhotoEntry) || new Set(photos.map(p => p.id)).size !== photos.length) throw damaged;
    const files = new Map<string, Blob>();
    for (const ride of rides) {
        const data = entries.get(filePath(ride));
        if (!data) throw damaged;
        files.set(ride.key, data);
    }
    const copies = photos.map(record => ({ record, image: entries.get(photoPath(record)) }));
    if (!copies.every((copy): copy is { record: PhotoEntry; image: Blob } => !!copy.image)) throw damaged;
    return { contents: { routes, versions: versions as Record<string, unknown[]>, rides, files, state: state as Partial<LibraryState> }, photos: copies, settings };
}

/** Adds the new records of a backup file. The routes and the rides come all or none: a route is new when its id is not
 * stored; a ride when its key is neither stored nor deleted. The settings come back only when the library was empty.
 * Then the photos come one at a time, each with a small picture that `small` makes from its copy: a photo is new when
 * its id is not stored, and it needs its ride in the library. A copy that `small` cannot open is left out. When a write
 * fails, the photos before it stay, and a second load adds the rest. */
export async function loadBackup(library: Library, file: Blob, small = smallPicture): Promise<{ routes: number; rides: number; photos: number; settings?: Record<string, unknown> }> {
    const { contents, photos, settings } = await readBackup(file);
    const { routes, rides, empty } = await library.merge(contents);
    const stored = new Set(await library.photoIds()), keys = new Set((await library.rides()).map(ride => ride.key));
    let added = 0;
    for (const { record, image } of photos) {
        if (stored.has(record.id) || !keys.has(record.ride)) continue;
        const picture = await small(image).catch(() => undefined);
        if (picture && await library.addPhoto({ ...record, image: image.slice(0, image.size, 'image/jpeg'), small: picture })) added++;
    }
    return { routes, rides, photos: added, ...empty && { settings } };
}
