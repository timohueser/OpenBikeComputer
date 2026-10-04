import { cumulative, hasEndpoints, itineraryDays, routeCoordinates, routingKey, storedPlan, type Trip } from './editor';

export interface Version {
    id: string;
    at: string;
    name?: string;
    summary: string;
    trip: Trip;
}

export function versionSummary(trip: Trip): string {
    const endpoints = trip.points.filter(p => p.kind === 'start' || p.kind === 'finish');
    if (!hasEndpoints(trip)) return endpoints.length ? `${endpoints[0].kind === 'start' ? 'Start' : 'Finish'} chosen` : 'Empty plan';
    const distance = trip.live && trip.routing?.key !== routingKey(trip) ? 'Distance pending' : `${cumulative(routeCoordinates(trip)).at(-1)!.toFixed(1)} km`;
    if (trip.mode === 'route') return `Single route · ${distance}`;
    const days = itineraryDays(trip).length;
    const pinned = trip.points.filter(p => p.kind === 'night').length;
    const parts = [`${days} ${days === 1 ? 'day' : 'days'}`];
    if (pinned) parts.push(`${pinned} ${pinned === 1 ? 'night' : 'nights'} pinned`);
    return [...parts, distance].join(' · ');
}

export function newVersion(trip: Trip, name?: string): Version {
    return { id: crypto.randomUUID(), at: new Date().toISOString(), summary: versionSummary(trip),
        ...(name?.trim() ? { name: name.trim() } : {}), trip: storedPlan(trip) };
}

export { planTitle } from './editor';
