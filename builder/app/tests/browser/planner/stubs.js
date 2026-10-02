/** The two hosts the planner build is pointed at (see `pretest:planner` in package.json). */
export const HOSTS = { tiles: 'tiles.test', api: 'api.test' };

/** The tile Worker serves these, and Cloudflare bills them; the VPS serves `api`. */
export const CLOUDFLARE = ['basemap', 'places', 'terrain', 'glyphs', 'sprites'];

const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);
const BOUNDS = [7.45, 47.5, 10.5, 49.85];
/** The zoom range of each vector archive, as the tile Worker's TileJSON gives it. */
const ZOOMS = { basemap: [0, 14], places: [11, 11] };
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
    return pathname.startsWith('/places') ? 'places' : 'basemap';
  }
  if (hostname === HOSTS.api) {
    if (pathname.endsWith('/v1/route')) return 'route';
    if (pathname.endsWith('/v1/overlays')) return 'overlays';
    return pathname.endsWith('/reverse') ? 'reverse' : 'query';
  }
  return `other:${hostname}`;
}

const metres = ([lon1, lat1], [lon2, lat2]) =>
  Math.hypot((lon2 - lon1) * 111_320 * Math.cos((lat1 * Math.PI) / 180), (lat2 - lat1) * 110_574);

/** A straight-line route through the points, in the answer format of `specs/route-api.md`. */
function route(points, profile, id) {
  const steps = 8;
  const geometry = [points[0]];
  const legs = [];
  for (let i = 1; i < points.length; i++) {
    const [a, b] = [points[i - 1], points[i]];
    const from = geometry.length - 1;
    for (let s = 1; s <= steps; s++) geometry.push([a[0] + ((b[0] - a[0]) * s) / steps, a[1] + ((b[1] - a[1]) * s) / steps]);
    legs.push({ from_index: from, to_index: geometry.length - 1 });
  }
  const distance = [0];
  for (let i = 1; i < geometry.length; i++) distance.push(distance[i - 1] + metres(geometry[i - 1], geometry[i]));
  const deltas = values => values.map((value, i) => value - (i ? values[i - 1] : 0));
  const [lon, lat] = [0, 1].map(axis => deltas(geometry.map(p => Math.round(p[axis] * 1e6))));
  const edges = geometry.length - 1;
  return {
    id, reason: id === 'primary' ? 'primary' : 'alternative', package: 'budget-test', profile,
    coordinates_udeg: lon.flatMap((x, i) => [x, lat[i]]),
    elevation_dm: deltas(geometry.map((_, i) => Math.round(3000 + 400 * Math.sin(i / 3)))),
    elapsed_s: deltas(distance.map(d => Math.round(d / 5))),
    surfaces: [['Paved', edges]],
    pushing: [[false, edges]],
    totals: {
      distance_m: distance[edges], ascent_m: 0, seconds: distance[edges] / 5,
      surface_m: [0, distance[edges], 0, 0, 0, 0], unknown_elevation_m: 0, pushing_m: 0,
    },
    legs,
    snap_truncated: false,
  };
}

/** The response for a request to one of the two hosts, or null when the path has no stub. */
export function respond(request) {
  const url = new URL(request.url());
  const kind = classify(url);
  const json = body => ({ headers: CORS, json: body });
  switch (kind) {
    case 'basemap':
    case 'places':
      return url.pathname === `/${kind}.json`
        ? json({ tilejson: '3.0.0', tiles: [`https://${HOSTS.tiles}/${kind}/{z}/{x}/{y}.mvt`], minzoom: ZOOMS[kind][0], maxzoom: ZOOMS[kind][1], bounds: BOUNDS })
        : { headers: CORS, status: 204 };
    case 'terrain':
      return { headers: CORS, contentType: 'image/png', body: PIXEL };
    case 'glyphs':
      return { headers: CORS, contentType: 'application/x-protobuf', body: Buffer.alloc(0) };
    case 'sprites':
      return url.pathname.endsWith('.png') ? { headers: CORS, contentType: 'image/png', body: PIXEL } : json({});
    case 'route': {
      const { points, profile, alternatives, alternatives_only } = request.postDataJSON();
      const routes = alternatives_only ? [] : [route(points, profile, 'primary')];
      if (alternatives || alternatives_only) routes.push(route(points, profile, 'alternative'));
      return json({ routes });
    }
    case 'overlays':
      return json({ type: 'FeatureCollection', features: [], package: 'budget-test', coverage: BOUNDS });
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
