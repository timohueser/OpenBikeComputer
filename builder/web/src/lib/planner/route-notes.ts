import type { Coordinate } from './geo';
import { closureLabel, type ClosureStretch, type Stretch } from './route-closures';
import { alpineLabel } from './route-difficulty';
import { gapLabel, type RouteGap } from './route-gaps';

/** `km` is where the note starts along the route; `length` is the length of its stretch. */
export interface RouteNote { icon: string; label: string; km: number; length?: number; coordinates: Coordinate[] }

/** The gaps, access notes and alpine stretches of a route, in route order. */
export function routeNotes(gaps: RouteGap[], closures: ClosureStretch[], alpine: Stretch[]): RouteNote[] {
    return [
        ...gaps.map(gap => ({ icon: 'flag', label: gapLabel(gap), km: gap.at, coordinates: gap.coordinates })),
        ...closures.map(stretch => ({ icon: 'lock', label: closureLabel(stretch), km: stretch.at, length: stretch.km, coordinates: stretch.coordinates })),
        ...alpine.map(stretch => ({ icon: 'mountain', label: alpineLabel, km: stretch.at, length: stretch.km, coordinates: stretch.coordinates })),
    ].sort((a, b) => a.km - b.km);
}

/** Names a note across plan edits that rebuild the list but keep the route. */
export const noteKey = (note: RouteNote) => `${note.icon} ${note.km} ${note.label}`;
