import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

// The climate and the snow archives as the tile service gives them: TileJSON with the archive metadata, and no tiles.
const archive = (name, metadata) => ({ tilejson: '3.0.0', tiles: [`https://tiles.test/${name}/{z}/{x}/{y}`], minzoom: 0, maxzoom: 7, bounds: [7.45, 47.5, 10.5, 49.85], ...metadata });
const SNOW = { snow_format: 1, first_season: 2015, seasons: 10, resolution_m: 500, attribution: 'Snow test data' };
const ARCHIVES = {
  climate: archive('climate', { first_year: 2015, years: 10, wet_day_mm: 1, attribution: 'Climate test data' }),
  snow: archive('snow', SNOW), 'snow-levels': archive('snow-levels', SNOW), 'snow-seasons': archive('snow-seasons', SNOW),
};

const overlap = (a, b) => a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
const inside = (inner, outer) => inner.x >= outer.x && inner.y >= outer.y && inner.x + inner.width <= outer.x + outer.width && inner.y + inner.height <= outer.y + outer.height;

/** The planner with the map settings open, at phone width or in a window of `width`, and the paths that it requests from then on. */
async function settings(page, width = 390) {
  const requested = [];
  await page.setViewportSize({ width, height: 844 });
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const { pathname } = new URL(request.url());
    requested.push(pathname);
    const [, name, tile] = /^\/([a-z-]+)(\.json|\/)/.exec(pathname) ?? [];
    if (ARCHIVES[name]) return route.fulfill(tile === '/' ? { headers: CORS_PREFLIGHT.headers, status: 204 } : { headers: CORS_PREFLIGHT.headers, json: ARCHIVES[name] });
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html#/planner');
  await page.getByRole('button', { name: 'Map settings' }).click();
  return { popup: page.getByRole('dialog', { name: 'Map settings' }), requested };
}

/** Five steps in from zoom 7, where the map starts. */
async function zoomTo12(page) {
  for (let i = 0; i < 5; i++) {
    await page.getByRole('button', { name: 'Zoom in' }).click();
    await page.waitForTimeout(500);
  }
}

test('the map settings show their pictures, and the legend of a data layer stands on the map above the date bar', async ({ page }) => {
  const { popup } = await settings(page);
  const pictures = popup.locator('img');
  await expect(pictures).toHaveCount(11);
  await expect.poll(() => pictures.evaluateAll(images => images.filter(image => !image.complete || !image.naturalWidth).length)).toBe(0);

  // Wind has the widest legend.
  await popup.getByRole('button', { name: 'Wind', exact: true }).click();
  const legend = page.getByRole('status').filter({ has: page.getByRole('img', { name: /^Wind colours/ }) });
  const dateBar = page.getByRole('group', { name: 'Layer date' });
  await expect(legend).toBeVisible();
  await expect(dateBar).toBeVisible();
  await expect(popup.getByRole('img', { name: /^Wind colours/ })).toHaveCount(0);
  const [map, legendBox, dateBox] = await Promise.all([page.locator('.map-area'), legend, dateBar].map(part => part.boundingBox()));
  expect(inside(legendBox, map) && inside(dateBox, map)).toBe(true);
  expect(overlap(legendBox, dateBox)).toBe(false);
  expect(legendBox.x >= dateBox.x && legendBox.x + legendBox.width <= dateBox.x + dateBox.width).toBe(true);
});

test('the legends of the signed routes, the roads, the slope angle and the snow layer stand over the date bar, clear of the map controls and of the scale bar', async ({ page }) => {
  const { popup, requested } = await settings(page);
  await popup.getByRole('button', { name: 'Slope angle', exact: true }).click();
  await popup.getByRole('button', { name: 'Snow', exact: true }).click();
  await page.keyboard.press('Escape');
  const routes = page.getByRole('status').filter({ hasText: 'Cycling routes' });
  const slope = page.getByRole('status').filter({ hasText: 'Slope angle' });
  const snow = page.getByRole('status').filter({ has: page.getByRole('img', { name: /^Snow colours/ }) });
  // The slope angle draws from zoom 12; the map starts at zoom 7.
  await expect(slope).toHaveText(/Slope angle · zoom in/);
  await expect(slope.getByRole('list', { name: 'Slope angle colours' }).getByRole('listitem')).toHaveCount(4);
  await zoomTo12(page);
  await expect(slope).not.toHaveText(/zoom in/);
  await expect(snow).toBeVisible();

  // The map requests the ways only while a layer of them is on.
  const ways = () => requested.filter(path => path.startsWith('/ways')).length;
  expect(ways()).toBe(0);
  await page.getByRole('button', { name: 'Map settings' }).click();
  await popup.getByRole('button', { name: 'Steepness', exact: true }).click();
  await page.keyboard.press('Escape');
  const roads = page.getByRole('status').filter({ hasText: 'Steepness' });
  await expect(roads.getByRole('list', { name: 'Steepness colours' }).getByRole('listitem')).toHaveCount(7);
  await expect.poll(ways).toBeGreaterThan(1);

  const stack = [routes, roads, slope, snow, page.getByRole('group', { name: 'Layer date' })];
  const [map, controls, scale, ...boxes] = await Promise.all([page.locator('.map-area'), page.getByLabel('Map controls'), page.locator('.maplibregl-ctrl-scale'), ...stack].map(part => part.boundingBox()));
  for (const box of boxes) expect(inside(box, map) && !overlap(box, controls) && !overlap(box, scale)).toBe(true);
  // From the top: the signed routes, the roads, the slope angle, the conditions layer, its date bar.
  expect(boxes.slice(1).every((box, i) => boxes[i].y + boxes[i].height <= box.y)).toBe(true);
});

// The map fits a route clear of the pills, so a pill whose height follows the zoom would move the zoom across 12 again.
// A note at the name must not move the swatches to another line. Windows of 480 px and of 1092 px give the stack of
// legends a width where the swatches of the slope angle, and those of the steepness, have room beside the name alone
// and not beside the name with its note.
for (const width of [390, 480, 1092, 1440]) {
  test(`the pills of the steepness and of the slope angle keep their height across zoom 12 in a window of ${width} px`, async ({ page }) => {
    const { popup } = await settings(page, width);
    await popup.getByRole('button', { name: 'Steepness', exact: true }).click();
    await popup.getByRole('button', { name: 'Slope angle', exact: true }).click();
    await page.keyboard.press('Escape');
    const pills = ['Steepness', 'Slope angle'].map(name => page.getByRole('status').filter({ hasText: name }));
    const heights = async () => (await Promise.all(pills.map(pill => pill.boundingBox()))).map(box => box.height);
    for (const pill of pills) await expect(pill).toHaveText(/· zoom in/);
    const far = await heights();
    await zoomTo12(page);
    for (const pill of pills) await expect(pill).not.toHaveText(/zoom in/);
    expect(await heights()).toEqual(far);
  });
}
