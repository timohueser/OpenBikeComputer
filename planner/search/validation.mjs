import contract from './query/contract.json' with { type: 'json' };

/** Input that a client must correct. Every other error is internal. */
export class RequestError extends Error {}
const fail = () => {
  throw new RequestError('Invalid planner search request.');
};
const coord = (p) =>
  Array.isArray(p) &&
  p.length === 2 &&
  p.every(Number.isFinite) &&
  Math.abs(p[0]) <= 180 &&
  Math.abs(p[1]) <= 90;
export function validateInput(input) {
  if (
    !input ||
    typeof input !== 'object' ||
    typeof input.q !== 'string' ||
    input.q.length > 240
  )
    fail();
  if (
    !Array.isArray(input.view) ||
    input.view.length !== 4 ||
    !input.view.every(Number.isFinite) ||
    input.view[0] >= input.view[2] ||
    input.view[1] >= input.view[3] ||
    Math.abs(input.view[0]) > 180 ||
    Math.abs(input.view[2]) > 180 ||
    Math.abs(input.view[1]) > 90 ||
    Math.abs(input.view[3]) > 90
  )
    fail();
  if (input.here && !coord(input.here)) fail();
  if (input.source !== undefined && (typeof input.source !== 'string' || !/^(?:[nwr][1-9]\d{0,13}|Q[1-9]\d{0,13})$/.test(input.source))) fail();
  if (
    input.limit !== undefined &&
    (!Number.isInteger(input.limit) || input.limit < 1 || input.limit > 100)
  )
    fail();
  if (
    input.startDate !== undefined &&
    (!/^\d{4}-\d{2}-\d{2}$/.test(input.startDate) ||
      !Number.isFinite(Date.parse(input.startDate)) ||
      new Date(input.startDate).toISOString().slice(0, 10) !== input.startDate)
  )
    fail();
  if (input.now !== undefined && !Number.isFinite(Date.parse(input.now)))
    fail();
  if (input.plan) {
    const p = input.plan;
    if (
      !Array.isArray(p.coordinates) ||
      p.coordinates.length > 20000 ||
      !p.coordinates.every(coord)
    )
      fail();
    if (
      !Array.isArray(p.days) ||
      p.days.length > 100 ||
      p.days.some(
        (d) =>
          !Number.isInteger(d.number) ||
          d.number < 1 ||
          !Number.isFinite(d.from) ||
          !Number.isFinite(d.to) ||
          d.from < 0 ||
          d.to < d.from,
      )
    )
      fail();
    for (const values of [p.hours, p.km])
      if (
        values &&
        (!Array.isArray(values) ||
          values.length !== p.coordinates.length ||
          values.some(
            (v, i) => !Number.isFinite(v) || v < 0 || (i && v < values[i - 1]),
          ))
      )
        fail();
    if (
      p.days.some(
        (d, i) =>
          i && (d.number <= p.days[i - 1].number || d.from < p.days[i - 1].to),
      )
    )
      fail();
    if (
      p.segments &&
      (!Array.isArray(p.segments) ||
        p.segments.length > 20000 ||
        p.segments.some(
          (s) =>
            !s ||
            typeof s.kind !== 'string' ||
            !Number.isFinite(s.from) ||
            !Number.isFinite(s.to) ||
            s.from < 0 ||
            s.to < s.from ||
            (s.ascent !== undefined && !Number.isFinite(s.ascent)) ||
            (s.gradient !== undefined && !Number.isFinite(s.gradient)),
        ))
    )
      fail();
    if (
      p.points &&
      (!Array.isArray(p.points) ||
        p.points.length > 500 ||
        p.points.some(
          (p) =>
            !coord(p.coordinate) ||
            typeof p.label !== 'string' ||
            typeof p.id !== 'string',
        ))
    )
      fail();
  }
  if (input.request) validateRequest(input.request);
  if (input.pointing) validateWhere(input.pointing);
}
const kinds = new Set([...Object.keys(contract.kinds), ...Object.keys(contract.cuisines)]);
const day = (d) =>
  ['today', 'tomorrow', 'every'].includes(d) || (Number.isInteger(d) && d > 0);
function quantity(q, units) {
  if (
    !q ||
    !Number.isFinite(q.value) ||
    q.value < 0 ||
    q.value > 100000 ||
    !units.includes(q.unit)
  )
    fail();
}
function along(a) {
  if (
    !a ||
    !contract.refs.includes(a.ref) ||
    !['at', 'from', 'to'].some((k) => a[k])
  )
    fail();
  for (const k of ['at', 'from', 'to']) if (a[k]) quantity(a[k], contract.units.along);
}
function point(p) {
  if (!p || typeof p !== 'object' || Array.isArray(p)) fail();
  if (p.name) {
    if (typeof p.name !== 'string' || p.name.length > 240) fail();
  } else if (p.kind) {
    if (!kinds.has(p.kind)) fail();
  } else if (p.here !== true && !['start', 'end'].includes(p.plan)) {
    if (p.day) {
      if (
        !day(p.day) ||
        p.day === 'every' ||
        (p.part && !contract.parts.includes(p.part))
      )
        fail();
    } else if (p.along) along(p.along);
    else fail();
  }
}
function validateWhere(w) {
  if (!w || typeof w !== 'object' || Array.isArray(w)) fail();
  if (w.anchor && !coord(w.anchor)) fail();
  if (w.scope && !contract.scopes.includes(w.scope)) fail();
  if (w.day && !day(w.day)) fail();
  if (w.part && !contract.parts.includes(w.part)) fail();
  if (w.near) {
    if (!Array.isArray(w.near) || w.near.length < 1 || w.near.length > 2)
      fail();
    w.near.forEach(point);
  }
  if (w.along) along(w.along);
  for (const k of ['before', 'after']) if (w[k]) point(w[k]);
}
export function validateRequest(r) {
  if (!contract.types.includes(r.type)) fail();
  if (
    r.type === 'places' &&
    (!Array.isArray(r.what) ||
      r.what.length < 1 ||
      r.what.length > 3 ||
      !r.what.every((k) => kinds.has(k)))
  )
    fail();
  if (
    r.type === 'place' &&
    (typeof r.name !== 'string' || !r.name || r.name.length > 240)
  )
    fail();
  if (
    (r.type === 'route' && !r.to) ||
    (r.type === 'end_day' && (!r.at || !day(r.day))) ||
    (['add_point', 'remove_point'].includes(r.type) && !r.point) ||
    (r.type === 'join' && (!Number.isInteger(r.day) || r.day < 1))
  )
    fail();
  if (
    r.type === 'stretches' &&
    (typeof r.what !== 'string' ||
      (!contract.stretches.includes(r.what) &&
        !(r.what.startsWith('gap:') && kinds.has(r.what.slice(4)))))
  )
    fail();
  if (r.where) validateWhere(r.where);
  for (const k of ['to', 'from', 'at', 'point', 'near']) if (r[k]) point(r[k]);
  if (r.via) {
    if (!Array.isArray(r.via) || r.via.length > 2) fail();
    r.via.forEach(point);
  }
  for (const k of ['radius', 'every', 'per_day', 'min'])
    if (r[k]) quantity(r[k], contract.units[k]);
  if (r.cuisine && !Object.hasOwn(contract.cuisines, r.cuisine)) fail();
  if (r.kind && !contract.point_kinds.includes(r.kind)) fail();
  if (
    r.ignored &&
    (!Array.isArray(r.ignored) ||
      r.ignored.length > 80 ||
      r.ignored.some((w) => typeof w !== 'string' || w.length > 240))
  )
    fail();
  if (
    r.days !== undefined &&
    (!Number.isInteger(r.days) || r.days < 1 || r.days > 100)
  )
    fail();
  if (r.type === 'split' && !!r.days === !!r.per_day) fail();
  if (r.bike && !contract.bikes.includes(r.bike)) fail();
  if (r.goal && !contract.goals.includes(r.goal)) fail();
  if (
    r.open &&
    !(
      r.open.now === true ||
      contract.weekdays.includes(r.open.weekday) ||
      day(r.open.day)
    )
  )
    fail();
}
