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

/** When a version was saved: "Today 14:05", "Yesterday 09:30" or "12 Oct 18:00". */
export function savedWhen(iso: string, now = Date.now()): string {
    const at = new Date(iso);
    const days = Math.round((new Date(now).setHours(0, 0, 0, 0) - new Date(iso).setHours(0, 0, 0, 0)) / 86400000);
    const day = days === 0 ? 'Today' : days === 1 ? 'Yesterday' : at.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
    return `${day} ${at.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}`;
}

/** The name of a version, or when it was saved. */
export const versionTitle = (version: Version, now = Date.now()) => version.name ?? savedWhen(version.at, now);

export { planTitle } from './editor';
