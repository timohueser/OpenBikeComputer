import { hasEndpoints, planView, type Trip } from './editor';
import type { RoutingLine } from './routing';

export interface Version {
    id: string;
    at: string;
    name?: string;
    summary: string;
    trip: Trip;
}

/** A short description of a plan; without its line, the distance is pending. */
export function versionSummary(trip: Trip, line: RoutingLine | undefined): string {
    const endpoints = trip.points.filter(p => p.kind === 'start' || p.kind === 'finish');
    if (!hasEndpoints(trip)) return endpoints.length ? `${endpoints[0].kind === 'start' ? 'Start' : 'Finish'} chosen` : 'Empty plan';
    const view = planView(trip, line);
    const distance = view.line ? `${view.summary.distance.toFixed(1)} km` : 'Distance pending';
    if (trip.mode === 'route') return `Single route · ${distance}`;
    const days = view.itinerary.length;
    const pinned = trip.points.filter(p => p.kind === 'night').length;
    const parts = [`${days} ${days === 1 ? 'day' : 'days'}`];
    if (pinned) parts.push(`${pinned} ${pinned === 1 ? 'night' : 'nights'} pinned`);
    return [...parts, distance].join(' · ');
}

export function newVersion(trip: Trip, line: RoutingLine | undefined, name?: string): Version {
    return { id: crypto.randomUUID(), at: new Date().toISOString(), summary: versionSummary(trip, line),
        ...(name?.trim() ? { name: name.trim() } : {}), trip };
}

export { planTitle } from './editor';
