import {
  addPointNear,
  hasEndpoints,
  pinNight,
  planView,
  routingKey,
  setSplit,
  type Trip,
  type RoutePoint,
} from '../editor';
import { coordinateAt, cumulative, nearestProgress, routeSlice, type Coordinate } from '../geo';
import type { RoutingLine } from '../routing';
import type { QueryChange, ResolvedPoint } from './types';

export type RouteBuilder = (
  points: ResolvedPoint[],
  bike: string,
  goal: string,
) => Promise<Coordinate[][]>;
const makePoint = (
  p: ResolvedPoint,
  kind: RoutePoint['kind'],
  line: Coordinate[],
): RoutePoint => ({
  id: crypto.randomUUID(),
  kind,
  coordinate: p.coordinate,
  label: p.label,
  progress: nearestProgress(line, p.coordinate),
  placeKind: p.kind,
});

/** All operations use a copy. A failed resolution never commits half of a sentence. */
export async function applyQueryChanges(
  original: Trip,
  changes: QueryChange[],
  buildRoute?: RouteBuilder,
  refreshRoute?: (trip: Trip) => Promise<RoutingLine>,
): Promise<Trip> {
  async function refresh(trip: Trip): Promise<Trip> {
    if (!trip.live || trip.routing?.key === routingKey(trip)) return trip;
    if (!refreshRoute) throw new Error('The routing engine must refresh the edited route.');
    return { ...trip, routing: await refreshRoute(trip) };
  }
  let trip = { ...original };
  for (const change of changes) {
    if (change.op !== 'route' && !hasEndpoints(trip))
      throw new Error('Choose a start and finish before editing the route.');
    // Reroute rebuilds the points from the stops, and a loop lists its start twice.
    if (trip.loop && change.op === 'reroute') throw new Error('Reroute does not work on a loop yet.');
    const { coordinates: line, total } = planView(trip);
    const ridingDay = (number: number) => {
      const day = planView(trip).itinerary.find(
        (d) => d.number === number && !d.rest,
      );
      if (!day) throw new Error(`Day ${number} is no longer a riding day.`);
      return day.ridingNumber;
    };
    if (change.op === 'add_point') {
      trip = addPointNear(
        trip,
        makePoint(
          change.point!,
          change.kind === 'pass' ? 'pass' : 'waypoint',
          line,
        ),
      );
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
        routeOrder: trip.routeOrder?.filter((id) => id !== change.id),
      };
    } else if (change.op === 'end_day') {
      const n = ridingDay(change.day!),
        p = change.point!;
      if (n >= planView(trip).days.length)
        throw new Error('The last day ends at the finish.');
      if (p.along !== undefined) {
        trip = {
          ...trip,
          points: trip.points.filter((p) => p.kind !== 'night' || p.night !== n),
        };
        trip = setSplit(trip, n, p.along / total);
        if (Math.abs((trip.splits?.[n] ?? -1) - p.along / total) > 1e-6)
          throw new Error(
            'That day end crosses another day boundary. Choose a point between the adjacent day ends.',
          );
      } else {
        trip = pinNight(trip, n, p.coordinate, p.label);
        trip.points.find((point) => point.night === n)!.placeKind = p.kind;
      }
    } else if (change.op === 'split' || change.op === 'join') {
      if (trip.points.some((p) => p.kind === 'night'))
        throw new Error(
          'Unpin the overnight places before changing the day count.',
        );
      if (trip.restAfter?.length)
        throw new Error('Remove rest days before changing the day count.');
      let boundaries = planView(trip).days
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
      // A loop keeps its start, which lists last again and holds the closing leg.
      const stops = [...planView(trip).stops].reverse();
      const kept = trip.loop ? stops.slice(0, -1) : stops;
      const drawn = (i: number) =>
        routeSlice(line, stops[i - 1].distance / total, stops[i].distance / total).slice(1, -1);
      const points = kept.map(({ point: p }, i, all) => ({
        ...p,
        kind: i === 0 ? 'start' : !trip.loop && i === all.length - 1 ? 'finish' : p.kind,
        progress: i === 0 ? 0 : 1 - p.progress,
        night: p.night ? trip.days - p.night : undefined,
        leg: i || trip.loop ? 'drawn' : undefined,
        drawn: i ? drawn(i) : trip.loop ? drawn(stops.length - 1) : undefined,
      })) as RoutePoint[];
      trip = {
        ...trip,
        points: [
          ...points,
          ...trip.points
            .filter((p) => p.kind === 'marker')
            .map((p) => ({ ...p, progress: 1 - p.progress })),
        ],
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
    } else if (change.op === 'route' || change.op === 'reroute') {
      if (!buildRoute)
        throw new Error(
          'Start the local routing engine to apply route requests. Place search and day edits are available now.',
        );
      if (change.op === 'reroute') {
        trip = await refresh(await reroute(trip, change, buildRoute));
        continue;
      }
      if (change.perDay?.unit === 'h')
        throw new Error(
          'Use kilometres per day until riding-time data is connected.',
        );
      const resolved = change.points!;
      const legs = await buildRoute(
        resolved,
        change.bike ?? trip.bike ?? 'touring',
        change.goal ?? 'balanced',
      );
      if (legs.length !== resolved.length - 1 || legs.some((l) => l.length < 2))
        throw new Error('The routing engine returned incomplete legs.');
      const all = legs.flatMap((l, i) => (i ? l.slice(1) : l));
      const points = resolved.map((p, i) => ({
        ...makePoint(
          p,
          i === 0 ? 'start' : i === resolved.length - 1 ? 'finish' : 'pass',
          all,
        ),
        leg: i ? ('drawn' as const) : undefined,
        drawn: i ? legs[i - 1].slice(1, -1) : undefined,
      }));
      const days =
        change.days ??
        (change.perDay?.unit === 'km'
          ? Math.ceil(cumulative(all).at(-1)! / change.perDay.value)
          : 1);
      if (days > 14)
        throw new Error(
          'This request needs more than 14 riding days. Increase the daily distance.',
        );
      trip = {
        ...trip,
        loop: undefined,
        points,
        routeOrder: points.slice(1, -1).map((p) => p.id),
        splits: undefined,
        restAfter: [],
        restNames: [],
        target: days,
        budget: 'days',
        days: Math.min(14, Math.max(1, days)),
        mode: days > 1 ? 'trip' : 'route',
        bike: (change.bike ?? trip.bike) as Trip['bike'],
      };
    }
    trip = await refresh(trip);
  }
  return trip;
}

/** Keep the route outside the selected interval, point identities, and day commitments. */
async function reroute(
  trip: Trip,
  change: QueryChange,
  build: RouteBuilder,
): Promise<Trip> {
  const { coordinates: line, stops, total } = planView(trip);
  const [from, to] = change.range ?? [0, total];
  if (from < 0 || to > total + 0.01 || to <= from)
    throw new Error('Choose a nonempty route section.');
  const inner = stops.filter((s) => s.distance > from && s.distance < to);
  const inputs = [
    { coordinate: coordinateAt(line, from / total), label: 'Section start' },
    ...inner.map((s) => ({
      coordinate: s.point.coordinate,
      label: s.point.label,
    })),
    { coordinate: coordinateAt(line, to / total), label: 'Section end' },
  ];
  const legs = await build(
    inputs,
    change.bike ?? trip.bike ?? 'touring',
    change.goal ?? 'balanced',
  );
  if (legs.length !== inputs.length - 1 || legs.some((l) => l.length < 2))
    throw new Error('The routing engine returned incomplete legs.');
  const section = legs.flatMap((l, i) => (i ? l.slice(1) : l)),
    sectionKm = cumulative(section).at(-1)!;
  const combined = [
    ...routeSlice(line, 0, from / total).slice(0, -1),
    ...section,
    ...routeSlice(line, to / total, 1).slice(1),
  ];
  const newTotal = cumulative(combined).at(-1)!;
  const remap = (km: number) =>
    km <= from
      ? km
      : km >= to
        ? km + sectionKm - (to - from)
        : from + ((km - from) / (to - from)) * sectionKm;
  const positions = new Map(stops.map((s) => [s.point.id, remap(s.distance)]));
  let at = from;
  for (let i = 0; i < inner.length; i++) {
    at += cumulative(legs[i]).at(-1)!;
    positions.set(inner[i].point.id, at);
  }
  const points = stops.map((s, i) => ({
    ...s.point,
    progress: positions.get(s.point.id)! / newTotal,
    leg: i ? ('drawn' as const) : undefined,
    drawn: i
      ? routeSlice(
          combined,
          positions.get(stops[i - 1].point.id)! / newTotal,
          positions.get(s.point.id)! / newTotal,
        ).slice(1, -1)
      : undefined,
  }));
  return {
    ...trip,
    points: [...points, ...trip.points.filter((p) => p.kind === 'marker')],
    splits: Object.fromEntries(
      Object.entries(trip.splits ?? {}).map(([n, p]) => [
        n,
        remap(p * total) / newTotal,
      ]),
    ),
    bike: (change.bike ?? trip.bike) as Trip['bike'],
  };
}
