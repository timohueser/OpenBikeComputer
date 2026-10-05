import {
  addPointNear,
  hasEndpoints,
  orderedRoutePoints,
  pinNight,
  planView,
  setSplit,
  type Trip,
  type RoutePoint,
} from '../editor';
import { routingKey, type RoutingLine } from '../routing';
import type { QueryChange, ResolvedPoint } from './types';

const makePoint = (p: ResolvedPoint, kind: RoutePoint['kind']): RoutePoint => ({
  id: crypto.randomUUID(),
  kind,
  coordinate: p.coordinate,
  label: p.label,
  placeKind: p.kind,
});
const presets: Record<string, string> = {
  balanced: 'Balanced',
  shortest: 'Shorter',
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
      trip = {
        ...trip,
        points: trip.points.filter((p) => p.id !== change.id),
        routeOrder: trip.routeOrder.filter((id) => id !== change.id),
      };
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
        trip = pinNight(trip, line, n, p.coordinate, p.label);
        trip.points.find((point) => point.night === n)!.placeKind = p.kind;
      }
    } else if (change.op === 'split' || change.op === 'join') {
      if (trip.points.some((p) => p.kind === 'night'))
        throw new Error(
          'Unpin the overnight places before changing the day count.',
        );
      if (trip.restAfter?.length)
        throw new Error('Remove rest days before changing the day count.');
      let boundaries = planView(trip, line).days
        .slice(0, -1)
        .map((d) => d.to * total);
      const [from, to] = change.range!;
      boundaries = boundaries.filter((k) => k <= from + 1e-6 || k >= to - 1e-6);
      if (change.op === 'split')
        boundaries.push(
          ...(change.boundaries ??
            Array.from(
              { length: change.count! - 1 },
              (_, i) => from + ((to - from) * (i + 1)) / change.count!,
            )),
        );
      boundaries.sort((a, b) => a - b);
      if (boundaries.length >= 14)
        throw new Error('The plan can have at most 14 riding days.');
      trip = {
        ...trip,
        mode: 'trip',
        days: boundaries.length + 1,
        budget: 'days',
        target: boundaries.length + 1,
        splits: Object.fromEntries(
          boundaries.map((km, i) => [i + 1, km / total]),
        ),
      };
    } else if (change.op === 'reverse') {
      if (trip.restAfter?.includes(trip.days))
        throw new Error(
          'Move the rest day at the finish before reversing this trip.',
        );
      // A point holds the leg that ends at it, so each leg moves to the point it now ends at.
      // A loop keeps its start, which lists last again and holds the closing leg.
      const order = orderedRoutePoints(trip).reverse();
      const kept = trip.loop ? order.slice(0, -1) : order;
      const legInto = (i: number) => ({
        leg: order[i - 1].leg,
        drawn: order[i - 1].drawn?.slice().reverse(),
      });
      const points = kept.map((p, i): RoutePoint => ({
        ...p,
        ...(i ? legInto(i) : trip.loop ? legInto(order.length - 1) : { leg: undefined, drawn: undefined }),
        kind: i === 0 ? 'start' : !trip.loop && i === kept.length - 1 ? 'finish' : p.kind,
        // Pinning finds a night by its id `night-N`.
        ...(p.night ? { id: `night-${trip.days - p.night}`, night: trip.days - p.night } : {}),
      }));
      trip = {
        ...trip,
        points: [...points, ...trip.points.filter((p) => p.kind === 'marker')],
        routeOrder: points.slice(1, trip.loop ? undefined : -1).map((p) => p.id),
        splits: Object.fromEntries(
          Object.entries(trip.splits ?? {}).map(([n, p]) => [
            trip.days - Number(n),
            1 - p,
          ]),
        ),
        restAfter: (trip.restAfter ?? []).map((n) => trip.days - n).reverse(),
        restNames: trip.restNames?.slice().reverse(),
      };
    } else if (change.op === 'route') {
      if (change.perDay?.unit === 'h')
        throw new Error(
          'Use kilometres per day until riding-time data is connected.',
        );
      const resolved = change.points!;
      const points = resolved.map((p, i) =>
        makePoint(p, i === 0 ? 'start' : i === resolved.length - 1 ? 'finish' : 'pass'),
      );
      trip = await refresh({
        ...trip,
        name: undefined,
        loop: undefined,
        points,
        routeOrder: points.slice(1, -1).map((p) => p.id),
        splits: undefined,
        restAfter: [],
        restNames: [],
        bike: (change.bike ?? trip.bike) as Trip['bike'],
        preset: change.goal ? presets[change.goal] : trip.preset,
      });
      const days = Math.max(1, change.days ??
        (change.perDay ? Math.ceil(planView(trip, line).total / change.perDay.value) : 1));
      if (days > 14)
        throw new Error(
          'This request needs more than 14 riding days. Increase the daily distance.',
        );
      trip = {
        ...trip,
        target: days,
        budget: 'days',
        days,
        mode: days > 1 ? 'trip' : 'route',
      };
    } else throw new Error('This edit is not available. Search again.');
    trip = await refresh(trip);
  }
  return { trip, line };
}
