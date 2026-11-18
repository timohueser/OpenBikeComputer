import type { RouteRecord, RouteSummary } from '../library/records';
import { summarize } from '../library/summary';
import { planTitle, planView, type Trip } from './editor';
import type { RoutingLine } from './routing';
import { isTrip } from './trip-validation';

/** A route of the library whose trip the planner can open. */
export interface Plan extends RouteRecord {
    trip: Trip;
}

export function newPlan(trip: Trip, name = ''): Plan {
    return { id: crypto.randomUUID(), revision: 0, name: name.trim().slice(0, 120), updatedAt: Date.now(), summary: null, trip };
}

export const isPlan = (route: RouteRecord): route is Plan => isTrip(route.trip);

// The simplified line depends only on the routed line; the numbers also follow the days and the bike of the trip.
const shapes = new WeakMap<RoutingLine, RouteSummary>();

/** The summary of a plan with its calculated line. */
export function planSummary(trip: Trip, line: RoutingLine): RouteSummary {
    const view = planView(trip, line), { distance, hours, ascent } = view.summary;
    const numbers = { lengthM: Math.round(distance * 1000), climbM: Math.round(ascent), seconds: Math.round(hours * 3600),
        bike: trip.bike ?? 'touring', days: Math.max(1, view.itinerary.length) };
    let shape = shapes.get(line);
    if (!shape) shapes.set(line, shape = summarize<RouteSummary>(line.coordinates.map(([lon, lat], i) => [lon, lat, line.elevation[i]]), numbers));
    return { ...shape, ...numbers };
}

// The companion app requires these keys; the web planner ignores them.
const withCompanionKeys = (trip: Trip) => ({ ...trip, budget: 'days', target: trip.days, limit: 0 });

export function exportPlan(plan: Plan): string {
    return JSON.stringify({ format: 'openbikecomputer-plan', version: 1, name: (plan.name || planTitle(plan.trip)).slice(0, 120),
        trip: withCompanionKeys(plan.trip) }, null, 2);
}

export function importPlan(text: string): Plan {
    const file = JSON.parse(text);
    if (!file || file.format !== 'openbikecomputer-plan' || file.version !== 1 || typeof file.name !== 'string'
        || !file.name.trim() || file.name.length > 120) throw new Error('Not an OpenBikeComputer plan file.');
    if (!isTrip(file.trip)) throw new Error('The file contains an invalid plan.');
    return newPlan(file.trip, file.name.trim());
}

export function downloadPlan(plan: Plan): void {
    const url = URL.createObjectURL(new Blob([exportPlan(plan)], { type: 'application/json' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = `${(plan.name || planTitle(plan.trip)).replace(/[^\p{L}\p{N} _-]/gu, '').slice(0, 80) || 'plan'}.obcplan`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}
