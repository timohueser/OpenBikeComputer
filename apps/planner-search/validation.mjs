const fail = () => {
  throw new Error('Invalid planner search request.');
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
  if (
    input.limit !== undefined &&
    (!Number.isInteger(input.limit) || input.limit < 1 || input.limit > 100)
  )
    fail();
  if (
    input.region !== undefined &&
    (typeof input.region !== 'string' || !/^[a-z][a-z0-9-]{0,63}$/.test(input.region))
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
    if (
      p.hours &&
      (!Array.isArray(p.hours) ||
        p.hours.length !== p.coordinates.length ||
        p.hours.some(
          (h, i) => !Number.isFinite(h) || h < 0 || (i && h < p.hours[i - 1]),
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
const kinds = new Set([
  'water',
  'drinking_water',
  'fountain',
  'spring',
  'water_tap',
  'sleep',
  'campsite',
  'shelter',
  'lodging',
  'hotel',
  'hostel',
  'guest_house',
  'motel',
  'hut',
  'resupply',
  'supermarket',
  'convenience',
  'bakery',
  'butcher',
  'marketplace',
  'fuel',
  'food',
  'cafe',
  'restaurant',
  'fast_food',
  'bar',
  'ice_cream',
  'pharmacy',
  'medical',
  'hospital',
  'doctor',
  'bike',
  'bike_shop',
  'repair_station',
  'charging',
  'toilets',
  'shower',
  'laundry',
  'atm',
  'transport',
  'train_station',
  'bus_stop',
  'ferry',
  'swimming',
  'lake',
  'beach',
  'swimming_pool',
  'sight',
  'viewpoint',
  'castle',
  'church',
  'monastery',
  'museum',
  'ruins',
  'waterfall',
  'pass',
  'summit',
  'tower',
  'bridge',
  'town',
  'pizza',
  'kebab',
]);
const day = (d) =>
  ['today', 'tomorrow', 'every'].includes(d) || (Number.isInteger(d) && d > 0);
function quantity(q, units = ['km', 'h', 'm', '%']) {
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
    !['km', 'start', 'end', 'here'].includes(a.ref) ||
    !['at', 'from', 'to'].some((k) => a[k])
  )
    fail();
  for (const k of ['at', 'from', 'to']) if (a[k]) quantity(a[k], ['km', 'h']);
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
        (p.part && !['start', 'middle', 'end'].includes(p.part))
      )
        fail();
    } else if (p.along) along(p.along);
    else fail();
  }
}
function validateWhere(w) {
  if (!w || typeof w !== 'object' || Array.isArray(w)) fail();
  if (w.anchor && !coord(w.anchor)) fail();
  if (w.scope && !['route', 'view', 'here'].includes(w.scope)) fail();
  if (w.day && !day(w.day)) fail();
  if (w.part && !['start', 'middle', 'end'].includes(w.part)) fail();
  if (w.near) {
    if (!Array.isArray(w.near) || w.near.length < 1 || w.near.length > 2)
      fail();
    w.near.forEach(point);
  }
  if (w.along) along(w.along);
  for (const k of ['before', 'after']) if (w[k]) point(w[k]);
}
export function validateRequest(r) {
  if (
    ![
      'places',
      'place',
      'route',
      'stretches',
      'end_day',
      'add_point',
      'remove_point',
      'split',
      'join',
      'reverse',
      'reroute',
      'none',
    ].includes(r.type)
  )
    fail();
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
      (![
        'climb',
        'descent',
        'steep',
        'unpaved',
        'unknown_surface',
        'pushing',
        'closure',
        'main_road',
      ].includes(r.what) &&
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
    if (r[k])
      quantity(
        r[k],
        k === 'radius' ? ['km'] : k === 'min' ? undefined : ['km', 'h'],
      );
  if (r.cuisine && !['pizza', 'kebab'].includes(r.cuisine)) fail();
  if (r.kind && !['visit', 'stop', 'pass'].includes(r.kind)) fail();
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
  if (r.bike && !['road', 'gravel', 'mtb', 'touring'].includes(r.bike)) fail();
  if (
    r.goal &&
    ![
      'balanced',
      'shortest',
      'least_climbing',
      'least_unpaved',
      'most_climbing',
    ].includes(r.goal)
  )
    fail();
  if (
    r.open &&
    !(
      r.open.now === true ||
      ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'].includes(
        r.open.weekday,
      ) ||
      day(r.open.day)
    )
  )
    fail();
}
