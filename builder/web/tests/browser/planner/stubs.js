import { createHash } from 'node:crypto';

/** The two hosts the planner build is pointed at (see `config.json`), and the host of its planner catalogue. */
export const HOSTS = { tiles: 'tiles.test', api: 'api.test', catalog: 'maps.openbikecomputer.com' };

/** The tile Worker serves these, and Cloudflare bills them; the VPS serves `api`. */
export const CLOUDFLARE = ['basemap', 'places', 'overlays', 'terrain', 'glyphs', 'sprites'];

const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);
const BOUNDS = [7.45, 47.5, 10.5, 49.85];
/** The zoom range of each vector archive, as the tile Worker's TileJSON gives it. */
const ZOOMS = { basemap: [0, 14], places: [11, 11], overlays: [6, 14] };
const CORS = {
  'access-control-allow-origin': '*',
  'access-control-allow-headers': 'content-type',
  'access-control-allow-methods': 'GET, POST, OPTIONS',
};

/** The request's category: the unit the budget counts. */
export function classify(url) {
  const { hostname, pathname } = url;
  if (hostname === HOSTS.tiles) {
    if (pathname.startsWith('/terrain')) return 'terrain';
    if (pathname.startsWith('/fonts')) return 'glyphs';
    if (pathname.startsWith('/sprites')) return 'sprites';
    if (pathname.startsWith('/places')) return 'places';
    if (pathname.startsWith('/routes/')) return 'routes';
    return pathname.startsWith('/overlays') ? 'overlays' : 'basemap';
  }
  if (hostname === HOSTS.api) {
    if (pathname.endsWith('/v1/route')) return 'route';
    if (pathname.endsWith('/v1/region')) return 'region';
    return pathname.endsWith('/reverse') ? 'reverse' : 'query';
  }
  if (hostname === HOSTS.catalog && pathname.startsWith('/route-pictures/')) return 'pictures';
  return `other:${hostname}`;
}

const metres = ([lon1, lat1], [lon2, lat2]) =>
  Math.hypot((lon2 - lon1) * 111_320 * Math.cos((lat1 * Math.PI) / 180), (lat2 - lat1) * 110_574);

/** A straight-line route through the points, in the answer format of `specs/route-api.md`. A leg position names its point. */
function route(points, profile, id) {
  const steps = 8;
  const geometry = [points[0]];
  const legs = [];
  for (let i = 1; i < points.length; i++) {
    const [a, b] = [points[i - 1], points[i]];
    const from = geometry.length - 1;
    for (let s = 1; s <= steps; s++) geometry.push([a[0] + ((b[0] - a[0]) * s) / steps, a[1] + ((b[1] - a[1]) * s) / steps]);
    legs.push({ from_index: from, to_index: geometry.length - 1, start: `budget-test:${a}`, end: `budget-test:${b}` });
  }
  const distance = [0];
  for (let i = 1; i < geometry.length; i++) distance.push(distance[i - 1] + metres(geometry[i - 1], geometry[i]));
  const totals = metres => ({ distance_m: metres, ascent_m: 0, seconds: metres / 5, surface_m: [0, metres, 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0 });
  for (const leg of legs) leg.totals = totals(distance[leg.to_index] - distance[leg.from_index]);
  const deltas = values => values.map((value, i) => value - (i ? values[i - 1] : 0));
  const [lon, lat] = [0, 1].map(axis => deltas(geometry.map(p => Math.round(p[axis] * 1e6))));
  const edges = geometry.length - 1;
  return {
    id, reason: id === 'primary' ? 'primary' : 'alternative', package: 'budget-test', profile,
    coordinates_udeg: lon.flatMap((x, i) => [x, lat[i]]),
    elevation_dm: deltas(geometry.map((_, i) => Math.round(3000 + 400 * Math.sin(i / 3)))),
    elapsed_s: deltas(distance.map(d => Math.round(d / 5))),
    totals: totals(distance[edges]),
    legs,
    snap_truncated: false,
  };
}

/** The zoom 9 cell of the route catalog around Freiburg that holds `CATALOG`; every other cell has no route. */
const ROUTE_CELL = '9-267-177';
/** Integer microdegrees: the first point, then the difference of each point to the one before (`specs/route-catalog.md`). */
const udeg = points => points.map(point => point.map(value => Math.round(value * 1e6)))
  .flatMap((point, i, all) => i ? [point[0] - all[i - 1][0], point[1] - all[i - 1][1]] : point);
/** A loop with a profile, a description of a few lines and an official page, stage 2 of a long route with a long description and no profile, the
 * long route, and a cycle route. */
const CATALOG = [
  { id: 101, kind: 'hiking', name: 'Schlossberg loop', rank: 1, loop: true, length_m: 1200, ascent_m: 150, descent_m: 150, hardest: 1,
    description: 'Over the castle hill and through the old town. '.repeat(12), operator: 'Test club', website: 'https://example.test/schlossberg', symbol: 'red:white:red_bar',
    cells: [ROUTE_CELL], line_udeg: udeg([[7.85, 47.99], [7.86, 47.995], [7.855, 48.0], [7.85, 47.99]]), via: [1, 2],
    profile_m: Array.from({ length: 14 }, (_, i) => 300 + 10 * Math.min(i, 13 - i)) },
  { id: 202, kind: 'hiking', name: 'Westweg 2', ref: 'WW', rank: 3, loop: false, length_m: 2500, ascent_m: 80, descent_m: 20, hardest: 0,
    cells: [ROUTE_CELL], line_udeg: udeg([[7.88, 47.98], [7.9, 47.985], [7.91, 48.0]]), via: [1], parent: 300, stage: 2,
    description: 'The stage follows the old trail through the forest and along the ridge to the next village. '.repeat(40) },
  { id: 300, kind: 'hiking', name: 'Westweg', rank: 3, loop: false, length_m: 5000, ascent_m: 200, descent_m: 100, cells: [ROUTE_CELL],
    stages: [201, 202], start_udeg: [7_800_000, 47_970_000] },
  { id: 303, kind: 'bicycle', name: 'Dreisam path', rank: 2, loop: false, length_m: 3000, ascent_m: 10, descent_m: 10, cells: [ROUTE_CELL],
    line_udeg: udeg([[7.83, 47.99], [7.87, 47.99]]), via: [] },
];

/** The live set of route pictures (`specs/route-pictures.md`) holds a cover of the first route of `CATALOG` only. */
const LOOK = '1|0715-1000|960|640';
const SET = '0123456789abcdef';
const COVERED = CATALOG[0];
const COVER = createHash('sha256').update(`${LOOK}|${COVERED.line_udeg.join(',')}`).digest('hex').slice(0, 16);
const PICTURES = {
  'manifest.json': { format: 1, set: SET, look: LOOK, months: { '07': '0715-1000' }, default_month: '07', credits: ['© Test terrain'] },
  [`sets/${SET}/cells/${ROUTE_CELL}.json`]: { format: 1, routes: { [COVERED.id]: COVER } },
  [`covers/${COVERED.id}/${COVER}.json`]: { x: [100, 860, 480, 100], y: [100, 300, 540, 100], eye: [140, 140, 140, 140] },
};

/** Bytes of a protobuf varint, and of a field with a varint or with bytes. */
const varint = value => { const out = []; for (; value > 127; value = Math.floor(value / 128)) out.push((value % 128) | 128); out.push(value); return out; };
const field = (number, value) => typeof value === 'number'
  ? [...varint(number * 8), ...varint(value)]
  : [...varint(number * 8 + 2), ...varint(value.length), ...value];
const text = value => [...Buffer.from(value)];
const zigzag = value => value < 0 ? -2 * value - 1 : 2 * value;

/** A Mapbox vector tile, version 2, with one layer `pois`: a point for each place, with its kind, name and position. */
function placeTile([z, x, y], places) {
  const extent = 4096, keys = ['kind', 'name', 'lon', 'lat'], values = [];
  const features = places.map(({ id, kind, name, lon, lat }) => {
    const column = ((lon + 180) / 360) * 2 ** z, row = ((1 - Math.asinh(Math.tan((lat * Math.PI) / 180)) / Math.PI) / 2) * 2 ** z;
    const tags = [kind, name, lon, lat].flatMap((value, key) => [key, values.push(value) - 1]);
    const geometry = [9, zigzag(Math.round((column - x) * extent)), zigzag(Math.round((row - y) * extent))];
    return field(2, [...field(1, id), ...field(2, tags.flatMap(varint)), ...field(3, 1), ...field(4, geometry.flatMap(varint))]);
  });
  const value = v => field(4, typeof v === 'string' ? field(1, text(v)) : [3 * 8 + 1, ...new Uint8Array(new Float64Array([v]).buffer)]);
  const layer = [...field(15, 2), ...field(1, text('pois')), ...features.flat(), ...keys.flatMap(key => field(3, text(key))), ...values.flatMap(value), ...field(5, extent)];
  return Buffer.from(field(3, layer));
}

/** Two restaurants 5 m apart where the map starts, in the Mercator middle of `BOUNDS`, a fountain 50 m east of them, and a
 * café 450 m south: at zoom 16, in the margin below the map. */
const PLACE_TILE = '11/1075/706';
const PLACES = placeTile([11, 1075, 706], [
  { id: 2 ** 44 + 1, kind: 'restaurant', name: 'Bistro', lon: 8.975, lat: 48.6887 },
  { id: 2 ** 44 + 2, kind: 'restaurant', name: 'Pizzeria', lon: 8.97507, lat: 48.6887 },
  { id: 2 ** 44 + 3, kind: 'fountain', name: 'Town fountain', lon: 8.97568, lat: 48.6887 },
  { id: 2 ** 44 + 4, kind: 'cafe', name: 'Hill café', lon: 8.975, lat: 48.6847 },
]);

/** The response for a request to one of the hosts, or null when the path has no stub. */
export function respond(request) {
  const url = new URL(request.url());
  const kind = classify(url);
  const json = body => ({ headers: CORS, json: body });
  switch (kind) {
    case 'basemap':
    case 'places':
    case 'overlays':
      return url.pathname === `/${kind}.json`
        ? json({ tilejson: '3.0.0', tiles: [`https://${HOSTS.tiles}/${kind}/{z}/{x}/{y}.mvt`], minzoom: ZOOMS[kind][0], maxzoom: ZOOMS[kind][1], bounds: BOUNDS, routing_package: 'budget-test' })
        : url.pathname === `/places/${PLACE_TILE}.mvt` ? { headers: CORS, contentType: 'application/x-protobuf', body: PLACES } : { headers: CORS, status: 204 };
    case 'terrain':
      return { headers: CORS, contentType: 'image/png', body: PIXEL };
    case 'glyphs':
      return { headers: CORS, contentType: 'application/x-protobuf', body: Buffer.alloc(0) };
    case 'sprites':
      return url.pathname.endsWith('.png') ? { headers: CORS, contentType: 'image/png', body: PIXEL } : json({});
    case 'pictures': {
      const path = url.pathname.slice('/route-pictures/'.length);
      if ([`covers/${COVERED.id}/${COVER}-0715-1000.webp`, `covers/${COVERED.id}/${COVER}-0715-1000-small.webp`].includes(path)) return { headers: CORS, contentType: 'image/png', body: PIXEL };
      return PICTURES[path] ? json(PICTURES[path]) : { headers: CORS, status: 404 };
    }
    case 'routes':
      return json({ format: 1, routes: url.pathname.endsWith(`/${ROUTE_CELL}.json`) ? CATALOG : [] });
    case 'route': {
      const { points, profile, alternatives, alternatives_only } = request.postDataJSON();
      const routes = alternatives_only ? [] : [route(points, profile, 'primary')];
      if (alternatives || alternatives_only) routes.push(route(points, profile, 'alternative'));
      return json({ routes });
    }
    case 'region':
      return json({ package: 'budget-test', profiles: ['gravel', 'hiking', 'mtb', 'road', 'touring', 'gravel/less-climbing', 'road/less-climbing', 'touring/less-climbing'] });
    case 'reverse':
      return json({ label: 'Teststraße 1, Freiburg' });
    case 'query': {
      const { request: parsed } = request.postDataJSON();
      return json({ type: 'places', request: parsed ?? { type: 'place', name: 'bakery' }, results: [] });
    }
    default:
      return null;
  }
}

export const CORS_PREFLIGHT = { status: 204, headers: CORS };
