import type { Coordinate } from './geo';
import { closureLabel, type ClosureStretch, type Stretch } from './route-closures';
import { alpineLabel } from './route-difficulty';
import { gapLabel, type RouteGap } from './route-gaps';

// The owner chose 2 km. OSM splits one pass road into many tagged pieces with short untagged pieces between them.
const MERGE_KM = 2;

/**
 * `km` is where the note starts along the route; `length` is the length of its stretch, summed over its pieces.
 * `end` is where the last piece ends, only on a note of more than one piece. `coordinates` has one line for each piece.
 */
export interface RouteNote { icon: string; label: string; km: number; end?: number; length?: number; coordinates: Coordinate[][] }

/**
 * The gaps, access notes and alpine stretches of a route, in route order. A stretch joins the last earlier note with
 * the same icon and label when it starts less than `MERGE_KM` after that note ends, whatever notes lie between them.
 */
export function routeNotes(gaps: RouteGap[], closures: ClosureStretch[], alpine: Stretch[]): RouteNote[] {
    const notes: RouteNote[] = [
        ...gaps.map(gap => ({ icon: 'flag', label: gapLabel(gap), km: gap.at, coordinates: [gap.coordinates] })),
        ...closures.map(stretch => ({ icon: 'lock', label: closureLabel(stretch), km: stretch.at, length: stretch.km, coordinates: [stretch.coordinates] })),
        ...alpine.map(stretch => ({ icon: 'mountain', label: alpineLabel, km: stretch.at, length: stretch.km, coordinates: [stretch.coordinates] })),
    ].sort((a, b) => a.km - b.km);
    const last = new Map<string, RouteNote>();
    return notes.filter(note => {
        const key = `${note.icon} ${note.label}`, before = last.get(key);
        if (note.length !== undefined && before?.length !== undefined && note.km - (before.end ?? before.km + before.length) < MERGE_KM) {
            before.end = note.km + note.length;
            before.length += note.length;
            before.coordinates.push(...note.coordinates);
            return false;
        }
        last.set(key, note);
        return true;
    });
}

/** Names a note across plan edits that rebuild the list but keep the route. */
export const noteKey = (note: RouteNote) => `${note.icon} ${note.km} ${note.label}`;
