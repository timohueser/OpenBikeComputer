import {
  addPointNear,
  hasEndpoints,
  maxRidingDays,
  removeRoutePoint,
  reverseTrip,
  repartitionDays,
  pinNight,
  planView,
  setSplit,
  withDays,
  type Trip,
  type RoutePoint,
} from '../editor';
import { nearestProgress, type Coordinate } from '../geo';
import { planPreset } from '../riding-profiles';
import { routingKey, type RoutingLine } from '../routing';
import type { Act } from './slots';
import type { QueryChange, ResolvedPoint, SearchContext, SearchPlace } from './types';

const makePoint = (p: ResolvedPoint, kind: RoutePoint['kind']): RoutePoint => ({
  id: crypto.randomUUID(),
  kind,
  coordinate: p.coordinate,
  label: p.label,
  placeKind: p.kind,
  placeId: p.source,
});
const presets: Record<string, string> = {
  balanced: 'Balanced',
  // The query model keeps its goal `shortest`; the planner has no Shorter preset.
  shortest: 'Balanced',
  least_climbing: 'Less climbing',
};

/**
 * The plan after all changes, with its line. `line` is the line of `original`. An edit of the points or the profile
 * calculates the line again before the next change. All operations use a copy. A failed resolution never commits half of
 * a sentence.
 */
export async function applyQueryChanges(
  original: Trip,
  line: RoutingLine | undefined,
  changes: QueryChange[],
  refreshRoute?: (trip: Trip) => Promise<RoutingLine>,
): Promise<{ trip: Trip; line: RoutingLine | undefined }> {
  let routed = original;
  async function refresh(next: Trip): Promise<Trip> {
    if (!hasEndpoints(next)) { line = undefined; routed = next; return next; }
    if (line && routingKey(next) === routingKey(routed)) return next;
    if (!refreshRoute) throw new Error('The routing engine must refresh the edited route.');
    line = await refreshRoute(next);
    routed = next;
    return next;
  }
  let trip = { ...original };
  for (const change of changes) {
    if (change.op !== 'route' && !hasEndpoints(trip))
      throw new Error('Choose a start and finish before editing the route.');
    const { total } = planView(trip, line);
    const ridingDay = (number: number) => {
      const day = planView(trip, line).itinerary.find(
        (d) => d.number === number && !d.rest,
      );
      if (!day) throw new Error(`Day ${number} is no longer a riding day.`);
      return day.ridingNumber;
    };
    if (change.op === 'add_point') {
      trip = addPointNear(trip, line, makePoint(change.point!, change.kind === 'pass' ? 'pass' : 'waypoint'));
    } else if (change.op === 'remove_point') {
      if (
        !trip.points.some(
          (p) => p.id === change.id && !['start', 'finish'].includes(p.kind),
        )
      )
        throw new Error('The selected point has changed. Search again.');
      trip = removeRoutePoint(trip, change.id!);
    } else if (change.op === 'end_day') {
      const n = ridingDay(change.day!),
        p = change.point!;
      if (n >= planView(trip, line).days.length)
        throw new Error('The last day ends at the finish.');
      if (p.along !== undefined) {
        // A pinned night of that day leaves the route, so the split uses the line without it.
        const pinned = `night-${n}`;
        trip = await refresh({
          ...trip,
          points: trip.points.filter((p) => p.id !== pinned),
          routeOrder: trip.routeOrder.filter((id) => id !== pinned),
        });
        trip = setSplit(trip, line, n, p.along / total);
        if (Math.abs((trip.splits?.[n] ?? -1) - p.along / total) > 1e-6)
          throw new Error(
            'That day end crosses another day boundary. Choose a point between the adjacent day ends.',
          );
      } else {
        trip = pinNight(trip, line, n, p.coordinate, p.label, undefined, { placeKind: p.kind, placeId: p.source });
      }
    } else if (change.op === 'split' || change.op === 'join') {
      const [from, to] = change.range!;
      const boundaries = change.op === 'join' ? [] : change.boundaries ??
        Array.from({ length: change.count! - 1 }, (_, i) => from + ((to - from) * (i + 1)) / change.count!);
      trip = repartitionDays(trip, line, [from, to], boundaries);
    } else if (change.op === 'reverse') {
      trip = reverseTrip(trip);
    } else if (change.op === 'route') {
      if (change.perDay?.unit === 'h')
        throw new Error(
          'Use kilometres per day until riding-time data is connected.',
        );
      const resolved = change.points!;
      const points = resolved.map((p, i) =>
        makePoint(p, i === 0 ? 'start' : i === resolved.length - 1 ? 'finish' : 'pass'),
      );
      const bike = (change.bike ?? trip.bike) as Trip['bike'];
      const preset = change.goal ? presets[change.goal] : trip.preset && planPreset(trip.bike, trip.preset);
      trip = await refresh({
        ...trip,
        name: undefined,
        loop: undefined,
        points,
        routeOrder: points.slice(1, -1).map((p) => p.id),
        splits: undefined,
        nightStay: undefined,
        restAfter: [],
        restNames: [],
        bike,
        preset: preset && planPreset(bike, preset),
      });
      const typed = change.days ??
        (change.perDay ? Math.ceil(planView(trip, line).total / change.perDay.value) : undefined);
      const days = Math.max(1, typed ?? 1);
      if (days > maxRidingDays)
        throw new Error(
          `This request needs more than ${maxRidingDays} riding days. Increase the daily distance.`,
        );
      trip = { ...withDays(trip, days), daysSet: typed === undefined ? undefined : true, mode: days > 1 ? 'trip' : 'route' };
    } else throw new Error('This edit is not available. Search again.');
    trip = await refresh(trip);
  }
  return { trip, line };
}

/** The riding days of the plan, and the one in which a place of the list lies: by its km along the route, else by the
 * nearest point of the route. */
function dayOf(place: SearchPlace, plan: SearchContext['plan']) {
  const riding = plan.days.filter((day) => !day.rest);
  const along = place.position?.along ?? nearestProgress(plan.coordinates, [place.lon, place.lat]) * (plan.km.at(-1) ?? 0);
  return { riding, day: riding.find((d) => along <= d.to) ?? riding.at(-1) };
}

/**
 * Why action `act` cannot apply to a place of the list (`reason`), or, for a stop, the point that the plan has at the
 * place already (`point`), in words. A route always applies: it goes from the rider or the start.
 */
export function placeCheck(act: Act, place: SearchPlace, plan: SearchContext['plan'], trip: Trip): { reason?: string; point?: string } {
  if (act === 'route') return {};
  const { riding, day } = dayOf(place, plan);
  if (!day || !hasEndpoints(trip)) return { reason: 'Choose a start and finish first.' };
  const kind = act === 'add_point' ? trip.points.find((p) => p.placeId === place.source)?.kind : undefined;
  const stop = riding.length > 1 ? `Stop on Day ${day.number}` : 'Stop on the route';
  const words: Partial<Record<RoutePoint['kind'], string>> = { start: 'Start', finish: 'Finish', night: `Night of Day ${day.number}`, waypoint: stop, pass: stop, via: stop, detour: stop };
  if (kind && words[kind]) return { point: words[kind] };
  return act === 'end_day' && day === riding.at(-1) ? { reason: 'The last day ends at the finish.' } : {};
}

/**
 * The change of action `act` on a place of the search list, and the line that says what it changed. A route goes from
 * the rider, else from the start of the plan, as the server routes. A stop and a day end go to the riding day in which
 * the place lies.
 */
export function placeChange(
  act: Act,
  place: SearchPlace,
  plan: SearchContext['plan'],
  here?: Coordinate,
): { changes: QueryChange[]; description: string } {
  const point: ResolvedPoint = { coordinate: [place.lon, place.lat], label: place.name, source: place.source, kind: place.kind };
  if (act === 'route') {
    const start = plan.points.find((p) => p.kind === 'start');
    const from = here ? { coordinate: here, label: 'Your location' } : start && { coordinate: start.coordinate, label: start.label };
    if (!from) throw new Error('Set a start, or allow location access, to route to a place.');
    return { changes: [{ op: 'route', points: [from, point] }], description: `The route now ends at ${place.name}` };
  }
  const { riding, day } = dayOf(place, plan);
  if (!day) throw new Error('Choose a start and finish before editing the route.');
  if (act === 'end_day') return { changes: [{ op: 'end_day', day: day.number, point }], description: `Day ${day.number} now ends at ${place.name}` };
  return {
    changes: [{ op: 'add_point', kind: 'stop', point }],
    description: riding.length > 1 ? `${place.name} is now a stop on Day ${day.number}` : `${place.name} is now a stop`,
  };
}
