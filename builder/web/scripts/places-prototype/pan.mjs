// node pan.mjs MODE AREA ZOOM DX [name] [extra]: pictures before and after a pan of DX screens, and a comparison of what both show.
import { AREAS, launch, open, settle, shot } from './lib.mjs';
const [mode, area, zoom, dx, name = '', extra = ''] = process.argv.slice(2);
const browser = await launch();
const { page, context } = await open(browser, { mode, center: AREAS[area], zoom: Number(zoom), extra });
const read = () => page.evaluate(() => ({ drawn: window.__places.drawn(), tiles: window.__places.loadedTiles.length, bounds: window.__places.map.getBounds().toArray().flat() }));
const before = await read();
if (name) await shot(page, `${name}-before`);
await page.evaluate(dx => window.__places.map.panBy([dx * 900, 0], { animate: false }), Number(dx));
await settle(page);
const after = await read();
if (name) await shot(page, `${name}-after`);
// The part of the map that both pictures show, less the width of a badge at the edges.
const west = Math.max(before.bounds[0], after.bounds[0]), east = Math.min(before.bounds[2], after.bounds[2]);
const inset = (east - west) * 0.06, south = before.bounds[1], north = before.bounds[3], inY = (north - south) * 0.04;
const key = item => `${item.layer} ${item.kinds} x${item.count} @${item.lon},${item.lat}`;
const shared = list => new Set(list.filter(item => item.lon > west + inset && item.lon < east - inset && item.lat > south + inY && item.lat < north - inY).map(key));
const a = shared(before.drawn), b = shared(after.drawn);
const gone = [...a].filter(k => !b.has(k)), added = [...b].filter(k => !a.has(k));
console.log(JSON.stringify({ mode, area, zoom, dx, extra, tilesBefore: before.tiles, tilesAfter: after.tiles, inBoth: a.size, gone: gone.length, added: added.length }));
for (const k of gone.slice(0, 8)) console.log('  before only:', k);
for (const k of added.slice(0, 8)) console.log('  after only: ', k);
await context.close(); await browser.close();
