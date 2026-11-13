import type { RouteRecord, RouteSummary } from '../library/records';
import { summarize } from '../library/summary';
import { planView, type Trip } from './editor';
import type { RoutingLine } from './routing';
import { isTrip } from './trip-validation';
import { planTitle, versionSummary, type Version } from './versions';

/** A route of the library whose trip the planner can open. Its versions are a separate record, written only when they
 * change. */
export interface Plan extends RouteRecord {
    trip: Trip;
}

export function newPlan(trip: Trip, name = ''): Plan {
    return { id: crypto.randomUUID(), revision: 0, name: name.trim().slice(0, 120), updatedAt: Date.now(), summary: null, trip };
}

export const isPlan = (route: RouteRecord): route is Plan => isTrip(route.trip);

export function isVersion(value: unknown): value is Version {
    const v = value as Partial<Version> | null;
    return !!v && isTrip(v.trip) && typeof v.id === 'string' && !!v.id && typeof v.at === 'string' && Number.isFinite(Date.parse(v.at))
        && new Date(v.at).toISOString() === v.at && (v.name === undefined || (typeof v.name === 'string' && v.name.length <= 120));
}

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

export function exportPlan(plan: Plan, versions: Version[]): string {
    return JSON.stringify({ format: 'openbikecomputer-plan', version: 1, name: (plan.name || planTitle(plan.trip)).slice(0, 120),
        open: versions.some(v => v.id === plan.open) ? plan.open : undefined, trip: withCompanionKeys(plan.trip), versions: versions.map(v => ({ ...v, trip: withCompanionKeys(v.trip) })) }, null, 2);
}

export function importPlan(text: string): { plan: Plan; versions: Version[] } {
    const file = JSON.parse(text);
    if (!file || file.format !== 'openbikecomputer-plan' || file.version !== 1 || typeof file.name !== 'string'
        || !file.name.trim() || file.name.length > 120 || !Array.isArray(file.versions)) throw new Error('Not an OpenBikeComputer plan file.');
    if (!isTrip(file.trip)) throw new Error('The file contains an invalid plan.');
    const ids = new Set<string>();
    const versions = file.versions.map((value: unknown): Version => {
        if (!isVersion(value) || ids.has(value.id)) throw new Error('The file contains an invalid version.');
        ids.add(value.id);
        return { id: value.id, at: value.at, ...(value.name ? { name: value.name } : {}), summary: versionSummary(value.trip, undefined), trip: value.trip };
    });
    const plan = newPlan(file.trip, file.name.trim());
    return { plan: ids.has(file.open) ? { ...plan, open: file.open } : plan, versions };
}

export function downloadPlan(plan: Plan, versions: Version[]): void {
    const url = URL.createObjectURL(new Blob([exportPlan(plan, versions)], { type: 'application/json' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = `${(plan.name || planTitle(plan.trip)).replace(/[^\p{L}\p{N} _-]/gu, '').slice(0, 80) || 'plan'}.obcplan`;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
}
