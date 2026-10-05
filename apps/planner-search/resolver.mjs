import {
  search,
  servesCuisine,
  distance,
  around,
  routePositions,
  distinct,
  rank,
  norm,
} from './web/engine.mjs';
import {
  centre,
  planKm,
  at,
  slice,
  dayRange,
  dayNumber,
  alongRange,
  boxes,
  hoursAt,
  crossesView,
  kmQuantity,
} from './web/geography.mjs';
import { routeResults } from './web/route-results.mjs';
import contract from './query/contract.json' with { type: 'json' };

/** The kind whose places a cuisine kind filters, or the kind itself. */
export const baseKind = (k) => (Object.hasOwn(contract.cuisines, k) ? contract.cuisines[k] : k);
/** The search-data kinds of `what`. A cuisine kind finds places of its kind that serve it. */
function placeKinds(what, cuisine) {
  return {
    kinds: [...new Set(what.flatMap((k) => contract.kinds[baseKind(k)].data))],
    cuisine: cuisine || what.find((k) => baseKind(k) !== k),
  };
}
const label = (value) => value.replaceAll('_', ' ');
const pointResult = (coordinate, name, extra = {}) => ({
  coordinate,
  label: name,
  ...extra,
});
function lineOf(context) {
  const line = context.plan?.coordinates;
  if (!line || line.length < 2) throw new Error('Add a route first.');
  return line;
}
function planRange(context) {
  lineOf(context);
  return [0, planKm(context).at(-1)];
}
function dayDate(day, context) {
  if (!context.startDate)
    throw new Error(
      'Set the trip start date to filter opening hours on a trip day.',
    );
  const n = dayNumber(day, context);
  dayRange(n, context);
  const date = new Date(`${context.startDate}T12:00:00Z`);
  date.setUTCDate(date.getUTCDate() + n - 1);
  return date.toISOString().slice(0, 10);
}

export function resolvePoint(db, point, context, focus = centre(context.view)) {
  if (point.name) {
    const found = search(db, {
      q: point.name,
      name: point.name,
      view: around(focus, 10),
    }).results;
    if (!found.length)
      throw new Error(`“${point.name}” is not in this data package.`);
    const p = found[0];
    return pointResult([p.lon, p.lat], p.name, {
      source: p.source,
      kind: p.kind,
      detail: [p.city, p.precision === 'street' ? 'Street location only' : '']
        .filter(Boolean)
        .join(' · '),
      alternatives: found
        .slice(1, 4)
        .map((p) => pointResult([p.lon, p.lat], p.name, { source: p.source })),
    });
  }
  if (point.kind) {
    const { kinds, cuisine } = placeKinds([point.kind]);
    const rows = categoryRows(db, kinds, around(focus, 25), focus, cuisine ? undefined : 1);
    const p = cuisine ? rows.find((p) => servesCuisine(p, cuisine)) : rows[0];
    if (!p || distance([p.lon, p.lat], focus) > 25)
      throw new Error(
        `No mapped ${label(point.kind)} within 25 km of this point.`,
      );
    return pointResult([p.lon, p.lat], p.name, {
      source: p.source,
      kind: p.kind,
    });
  }
  if (point.here) {
    if (!context.here) throw new Error('Set your location to use “here”.');
    return pointResult(context.here, 'Your location');
  }
  const line = lineOf(context),
    ds = planKm(context),
    total = ds.at(-1);
  if (point.plan)
    return pointResult(
      point.plan === 'start' ? line[0] : line.at(-1),
      `Route ${point.plan}`,
    );
  if (point.day) {
    const [a, b] = dayRange(point.day, context),
      km =
        point.part === 'start' ? a : point.part === 'middle' ? (a + b) / 2 : b;
    return pointResult(
      at(line, km, ds),
      `Day ${dayNumber(point.day, context)} ${point.part || 'end'}`,
      { along: km },
    );
  }
  if (point.along) {
    const [km] = alongRange(point.along, [0, total], context);
    return pointResult(at(line, km, ds), `${km.toFixed(1)} km`, { along: km });
  }
  throw new Error('Choose a point.');
}

function scope(db, request, context) {
  const w = request.where || context.pointing || { scope: 'view' },
    radius = request.radius?.value,
    ds = planKm(context);
  let range = null,
    radial = null,
    line = null,
    focus = centre(context.view),
    bounds = [context.view],
    area = 'In this map view';
  if (
    radius !== undefined &&
    (!w.scope || w.scope === 'view') &&
    !w.day &&
    !w.along &&
    !w.near &&
    !w.before &&
    !w.after
  ) {
    radial = radius;
    bounds = [around(focus, radius)];
    area = `Within ${radius} km of the map centre`;
  }
  if (w.anchor) {
    focus = w.anchor;
    radial = radius ?? 3;
    bounds = [around(focus, radial)];
    area = `Within ${radial} km of the selected point`;
  }
  if (w.scope === 'here') {
    if (!context.here) throw new Error('Set your location to search near you.');
    focus = context.here;
    radial = radius ?? 5;
    bounds = [around(focus, radial)];
    area = `Within ${radial} km of your location`;
  }
  if (w.scope === 'route' || w.day || w.along || w.before || w.after) {
    line = lineOf(context);
    range =
      w.day && w.day !== 'every'
        ? dayRange(w.day, context)
        : planRange(context);
    area =
      w.day && w.day !== 'every'
        ? `Along Day ${dayNumber(w.day, context)}`
        : 'Along the route';
    if (w.along) {
      range = alongRange(w.along, range, context);
      area = `Route km ${range[0].toFixed(1)}–${range[1].toFixed(1)}`;
    }
    for (const key of ['before', 'after'])
      if (w[key]) {
        const p = resolvePoint(
          db,
          w[key],
          context,
          at(line, range[key === 'after' ? 0 : 1], ds),
        );
        const position = routePositions(line, ds)(p.coordinate);
        if (position.distance > 5)
          throw new Error(`“${p.label}” is more than 5 km from the route.`);
        range[key === 'after' ? 0 : 1] =
          key === 'after'
            ? Math.max(range[0], position.along)
            : Math.min(range[1], position.along);
        area += ` ${key} ${p.label}`;
      }
    if (range[0] > range[1])
      throw new Error('The requested points are in the opposite route order.');
    if (w.part || range[0] === range[1]) {
      const km =
        w.part === 'start'
          ? range[0]
          : w.part === 'middle'
            ? (range[0] + range[1]) / 2
            : range[1];
      focus = at(line, km, ds);
      radial = radius ?? 3;
      bounds = [around(focus, radial)];
      line = null;
      area = `Within ${radial} km of ${w.day ? `Day ${dayNumber(w.day, context)} ${w.part}` : w.part ? `the route ${w.part}` : `km ${km.toFixed(1)}`}`;
    } else {
      line = slice(line, ...range, ds);
      bounds = boxes(line, radius ?? 1);
      focus = line[0];
    }
  }
  if (w.near?.length) {
    const points = w.near.map((p) => resolvePoint(db, p, context, focus));
    if (points.length === 1) {
      focus = points[0].coordinate;
      radial = radius ?? 5;
      bounds = [around(focus, radial)];
      area = `Within ${radial} km of ${points[0].label}`;
    } else {
      const route = lineOf(context),
        position = routePositions(route, ds),
        positions = points.map((p) => position(p.coordinate));
      if (positions.some((p) => p.distance > 5))
        throw new Error('Both points must be within 5 km of the route.');
      const nearRange = positions.map((p) => p.along).sort((a, b) => a - b);
      range = range
        ? [Math.max(range[0], nearRange[0]), Math.min(range[1], nearRange[1])]
        : nearRange;
      if (range[0] > range[1]) throw new Error('These areas do not overlap.');
      line = slice(route, ...range, ds);
      bounds = boxes(line, radius ?? 1);
      area = `Between ${points[0].label} and ${points[1].label}`;
    }
  }
  return { w, range, line, radial, focus, bounds, area, radius: radius ?? 1 };
}

function categoryRows(db, kinds, bounds, focus, limit = 2001) {
  const x = focus[0],
    y = focus[1],
    cos = Math.cos((y * Math.PI) / 180) ** 2;
  return db.places([{
    sql: `SELECT p.id,(p.lon-?)*(p.lon-?)*?+(p.lat-?)*(p.lat-?) AS _distance,p.source
    FROM {c}.spatial s JOIN {c}.place_records p ON p.id=s.id
    WHERE s.east>=? AND s.north>=? AND s.west<=? AND s.south<=?
    AND p.kind IN (${kinds.map(() => '?').join(',')})`,
    params: [x, x, cos, y, y, ...bounds, ...kinds],
    order: ['_distance', 'source'],
    limit,
    bounds,
  }]);
}
// `all` adds every sorted match with its route position for internal callers.
export function findPlaces(db, request, context, { all = false } = {}) {
  const where = request.where || context.pointing;
  if (where?.day === 'every' && where.part) {
    const days = context.plan?.days?.filter((d) => !d.rest) || [];
    if (!days.length) throw new Error('Add a trip first.');
    const answers = days.map((d) =>
      findPlaces(
        db,
        {
          ...request,
          where: { ...where, day: d.number },
          open:
            request.open?.day === 'every' ? { day: d.number } : request.open,
        },
        context,
        { all: true },
      ),
    );
    const results = distinct(
      rank(
        answers
          .flatMap((a) => a.all)
          .map((p) => ({ ...p, score: -p.position.along })),
      ),
    );
    return {
      type: 'places',
      results: routeResults(results, planRange(context), context.limit || 20),
      hasMore: results.length > (context.limit || 20),
      area: `Near every day ${where.part}`,
      note: [...new Set(answers.map((a) => a.note).filter(Boolean))].join(' '),
      truncated: answers.some((a) => a.truncated),
      ...(all ? { all: results } : {}),
    };
  }
  const sc = scope(db, request, context),
    { kinds, cuisine } = placeKinds(request.what, request.cuisine);
  if (request.open?.day)
    context = { ...context, openDate: dayDate(request.open.day, context) };
  const implicit =
    (!request.where && !context.pointing) ||
    (Object.keys(sc.w).length === 1 && sc.w.scope === 'view');
  const alongRoute = sc.line || (
    implicit &&
    context.plan?.coordinates?.length > 1 &&
    crossesView(context.plan.coordinates, context.view)
  );
  // A route position costs a search of the line, so only route scopes measure it.
  const position =
    (sc.range || alongRoute || all) && context.plan?.coordinates?.length > 1
      ? routePositions(context.plan.coordinates, planKm(context))
      : null;
  const found = new Map();
  let truncated = false,
    unknown = 0;
  function collect(bounds) {
    for (const box of bounds) {
      const rows = categoryRows(db, kinds, box, sc.focus);
      if (rows.length > 2000) truncated = true;
      for (const p of rows.slice(0, 2000)) found.set(p.source, p);
      if (found.size >= 20000) {
        truncated = true;
        break;
      }
    }
  }
  const filter = () => {
    unknown = 0;
    return [...found.values()].flatMap((p) => {
      const km = distance([p.lon, p.lat], sc.focus),
        pos = position?.([p.lon, p.lat]) ?? null;
      if (
        (sc.radial !== null && km > sc.radial) ||
        (sc.line &&
          (pos.distance > sc.radius ||
            pos.along < sc.range[0] - 1e-7 ||
            pos.along > sc.range[1] + 1e-7))
      )
        return [];
      if (cuisine && !servesCuisine(p, cuisine)) return [];
      const opening = request.open ? context.openingState(p, request.open, context) : null;
      if (opening === 'unknown') unknown++;
      if (opening && opening !== 'open') return [];
      return [
        {
          ...p,
          distance: km,
          position: pos,
          score: alongRoute ? -pos.along : -km,
          precision: 'place',
          opening,
          why: {
            category: p.kind,
            order: alongRoute
              ? 'Spread along route, favouring nearby places'
              : 'Distance from search centre',
          },
        },
      ];
    });
  };
  collect(sc.bounds);
  let results = filter();
  const notes = [];
  if (!results.length && implicit && request.radius === undefined) {
    for (const km of [5, 15, 50]) {
      if (
        km < distance(centre(context.view), [context.view[2], context.view[3]])
      )
        continue;
      found.clear();
      unknown = 0;
      sc.radial = km;
      collect([around(sc.focus, km)]);
      results = filter();
      if (results.length) {
        sc.area = `Within ${km} km of the map centre`;
        notes.push(`No match in this map view. Search widened to ${km} km.`);
        break;
      }
    }
  }
  if (unknown)
    notes.push(
      `${unknown} mapped places have unknown opening hours and are excluded.`,
    );
  if (request.open)
    notes.push(
      'Opening hours come from map tags. They do not predict arrival time.',
    );
  if (truncated)
    notes.push(
      'This area has more matches than can be checked at once. Zoom in or narrow the request.',
    );
  const sorted = distinct(rank(results)),
    limit = context.limit || 20;
  return {
    type: 'places',
    results: alongRoute ? routeResults(sorted, sc.range || planRange(context), limit) : sorted.slice(0, limit),
    hasMore: sorted.length > limit,
    area: sc.area,
    note: notes.join(' '),
    truncated,
    ...(all ? { all: sorted } : {}),
  };
}

export function resolve(db, request, context) {
  if (request.type === 'none' || request.type === 'place') {
    const near = request.near ? resolvePoint(db, request.near, context) : null;
    const view = near ? around(near.coordinate, 5) : context.view;
    return {
      type: 'places',
      ...search(db, {
        q: context.q,
        name: request.name || context.q,
        view,
        withinKm: request.near ? 5 : undefined,
        limit: context.limit,
      }),
      ...(near ? { area: `Within 5 km of ${near.label}` } : {}),
    };
  }
  if (request.type === 'places') return findPlaces(db, request, context);
  const line = context.plan?.coordinates || [],
    ds = planKm(context),
    total = ds.at(-1);
  const changes = [];
  let description = '';
  // The longest stretches first. Only the shown ones get coordinates: each slice reads the line.
  const longest = (stretches, name) =>
    stretches
      .sort((a, b) => b.to - b.from - (a.to - a.from))
      .slice(0, 20)
      .map((s) => ({ ...s, label: name, coordinates: slice(line, s.from, s.to, ds) }));
  if (request.type === 'route') {
    const from = resolvePoint(
      db,
      request.from || (context.here ? { here: true } : { plan: 'start' }),
      context,
    );
    const via = (request.via || []).map((p) =>
      resolvePoint(db, p, context, from.coordinate),
    );
    const to = resolvePoint(
      db,
      request.to,
      context,
      via.at(-1)?.coordinate || from.coordinate,
    );
    changes.push({
      op: 'route',
      points: [from, ...via, to],
      bike: request.bike,
      goal: request.goal,
      days: request.days,
      perDay: request.per_day,
    });
    description = `Route from ${from.label} to ${to.label}${via.length ? ' via ' + via.map((p) => p.label).join(', ') : ''}`;
  } else if (request.type === 'end_day') {
    lineOf(context);
    const days =
      request.day === 'every'
        ? context.plan.days
            .filter((d) => !d.rest && d.to < total)
            .map((d) => d.number)
        : [dayNumber(request.day, context)];
    for (const day of days) {
      const range = dayRange(day, context);
      if (range[1] === total)
        throw new Error(
          'The final day ends at the route finish. Choose an earlier day.',
        );
      const p = resolvePoint(db, request.at, context, at(line, range[1], ds));
      changes.push({ op: 'end_day', day, point: p });
    }
    description = `Move ${changes.length === 1 ? `Day ${changes[0].day} end to ${changes[0].point.label}` : 'the day ends'}`;
  } else if (request.type === 'add_point') {
    const sc = scope(db, request, context),
      range = sc.range || planRange(context);
    if (request.every) {
      if (request.every.value <= 0)
        throw new Error('Choose a repeat distance greater than zero.');
      const span = request.every.unit === 'h'
        ? hoursAt(range[1], context) - hoursAt(range[0], context)
        : range[1] - range[0];
      if (Math.ceil(span / request.every.value) - 1 > 50)
        throw new Error('This checks more than 50 stop intervals. Use a larger interval.');
      let km = range[0];
      while (km < range[1]) {
        if (
          (request.every.unit === 'km' &&
            km + request.every.value >= range[1]) ||
          (request.every.unit === 'h' &&
            hoursAt(km, context) + request.every.value >=
              hoursAt(range[1], context))
        )
          break;
        const next = alongRange(
          { ref: 'start', at: request.every },
          [km, range[1]],
          context,
        )[0];
        if (next <= km) throw new Error('The repeat interval does not advance along the route.');
        if (next >= range[1]) break;
        km = next;
        const point = resolvePoint(db, request.point, context, at(line, km, ds));
        if (
          !changes.some(
            (c) => distance(c.point.coordinate, point.coordinate) < 0.02,
          )
        )
          changes.push({
            op: 'add_point',
            kind: request.kind || 'stop',
            point,
          });
        if (changes.length > 50)
          throw new Error(
            'This adds more than 50 stops. Use a larger interval.',
          );
      }
    } else
      changes.push({
        op: 'add_point',
        kind: request.kind || 'visit',
        point: resolvePoint(db, request.point, context, sc.focus),
      });
    if (!changes.length) throw new Error('No stop fits inside that interval.');
    description =
      changes.length === 1
        ? `Add ${changes[0].point.label}`
        : `Add ${changes.length} stops`;
  } else if (request.type === 'remove_point') {
    const points = (context.plan?.points || []).filter(
      (p) => !['start', 'finish'].includes(p.kind),
    );
    const matched = request.point.name
      ? points.filter((p) => norm(p.label) === norm(request.point.name))
      : request.point.kind
        ? points.filter((p) => placeKinds([request.point.kind]).kinds.includes(p.placeKind))
        : points.filter(
            (p) =>
              distance(
                p.coordinate,
                resolvePoint(db, request.point, context).coordinate,
              ) < 0.05,
          );
    if (matched.length !== 1)
      throw new Error(
        matched.length
          ? 'More than one point matches. Select it on the map.'
          : 'No plan point matches this request.',
      );
    changes.push({ op: 'remove_point', id: matched[0].id });
    description = `Remove ${matched[0].label}`;
  } else if (request.type === 'split') {
    const sc = scope(db, request, context),
      range = sc.range || planRange(context);
    let count = request.days;
    if (request.per_day) {
      if (request.per_day.value <= 0)
        throw new Error('Choose a daily target greater than zero.');
      const span =
        request.per_day.unit === 'h'
          ? hoursAt(range[1], context) - hoursAt(range[0], context)
          : range[1] - range[0];
      count = Math.ceil(span / request.per_day.value);
    }
    if (count < 1 || count > 14)
      throw new Error('Choose between 1 and 14 riding days.');
    const boundaries =
      request.per_day?.unit === 'h'
        ? Array.from({ length: count - 1 }, (_, i) =>
            kmQuantity(
              {
                unit: 'h',
                value:
                  hoursAt(range[0], context) +
                  ((hoursAt(range[1], context) - hoursAt(range[0], context)) *
                    (i + 1)) /
                    count,
              },
              context,
            ),
          )
        : undefined;
    changes.push({ op: 'split', range, count, boundaries });
    description = `Split ${request.where ? 'this section' : 'the route'} into ${count} days`;
  } else if (request.type === 'join') {
    const a = dayRange(request.day, context),
      b = dayRange(request.day + 1, context);
    changes.push({ op: 'join', day: request.day, range: [a[0], b[1]] });
    description = `Join Days ${request.day} and ${request.day + 1}`;
  } else if (request.type === 'reverse') {
    lineOf(context);
    changes.push({ op: 'reverse' });
    description = 'Reverse the route';
  } else if (request.type === 'stretches') {
    const sc = scope(db, request, context),
      range = sc.range || planRange(context);
    if (request.what.startsWith('gap:')) {
      if (request.min && request.min.unit !== 'km')
        throw new Error('Use kilometres for a mapped-place gap.');
      const found = findPlaces(
        db,
        {
          type: 'places',
          what: [request.what.slice(4)],
          where: request.where || { scope: 'route' },
        },
        context,
        { all: true },
      );
      if (found.truncated)
        throw new Error(
          'Narrow this section to check all mapped places before measuring gaps.',
        );
      const positions = [
        range[0],
        ...found.all
          .map((p) => p.position.along)
          .filter((k) => k >= range[0] && k <= range[1])
          .sort((a, b) => a - b),
        range[1],
      ];
      const gaps = positions
        .slice(1)
        .map((to, i) => ({ from: positions[i], to }))
        .filter((s) => s.to - s.from >= (request.min?.value || 0));
      return {
        type: 'stretches',
        stretches: longest(gaps, `No mapped ${label(request.what.slice(4))}`),
        area: sc.area,
        note: 'Gaps use mapped places within 1 km of the line. Missing map data can make a gap look longer.',
      };
    }
    const segments = context.plan.segments;
    if (!segments)
      throw new Error(
        'This route has no verified surface, gradient or access data yet.',
      );
    const found = segments
      .filter(
        (s) =>
          s.kind === request.what && s.to >= range[0] && s.from <= range[1],
      )
      .map((s) => ({
        ...s,
        from: Math.max(s.from, range[0]),
        to: Math.min(s.to, range[1]),
      }))
      .filter(
        (s) =>
          !request.min ||
          (request.min.unit === 'km'
            ? s.to - s.from
            : request.min.unit === 'm'
              ? s.ascent
              : s.gradient) >= request.min.value,
      );
    return { type: 'stretches', stretches: longest(found, label(request.what)), area: sc.area };
  } else throw new Error('This request type is not supported.');
  return {
    type: 'change',
    changes,
    description,
    area: 'Current plan',
    results: [],
    note: 'Review the change, then Apply. Undo restores the previous plan.',
  };
}
