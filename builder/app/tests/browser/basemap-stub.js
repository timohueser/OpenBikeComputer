/** The host the journeys build the planner basemap against (see `pretest` in package.json). It is decoration; no journey needs a tile. */
export const BASEMAP = 'basemap.test';

/** One transparent pixel, so the sprite sheet loads without a failure. */
const PIXEL = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);

/** The response for a request to `BASEMAP`: a TileJSON, an empty sprite sheet, and no tiles. */
export function basemapResponse(url) {
  const headers = { 'access-control-allow-origin': '*' };
  switch (url.pathname) {
    case '/basemap.json':
      return { headers, json: { tilejson: '3.0.0', tiles: [`https://${BASEMAP}/{z}/{x}/{y}.mvt`], maxzoom: 14 } };
    case '/sprites/light.json':
      return { headers, json: {} };
    case '/sprites/light.png':
      return { headers, contentType: 'image/png', body: PIXEL };
    default:
      return { headers, status: 204 };
  }
}
