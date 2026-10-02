import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import { CLOUDFLARE, CORS_PREFLIGHT, classify, respond } from './stubs.js';

const budget = JSON.parse(readFileSync(new URL('./request-budget.json', import.meta.url), 'utf8'));

/** Counts every request the page and its workers send to a public host, and answers it from `stubs.js`. */
async function countRequests(page) {
  const counts = {};
  const order = [];
  const state = { last: Date.now() };
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const kind = classify(new URL(request.url()));
    counts[kind] = (counts[kind] ?? 0) + 1;
    order.push(kind);
    state.last = Date.now();
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
  const phases = [];
  let before = {};
  return {
    counts, phases, order,
    idle: async () => { do await page.waitForTimeout(500); while (Date.now() - state.last < 500); },
    /** Closes the current phase: what it requested, by category. */
    phase(name) {
      const delta = Object.fromEntries(Object.entries(counts).map(([kind, n]) => [kind, n - (before[kind] ?? 0)]).filter(([, n]) => n));
      phases.push(`${name} ${JSON.stringify(delta)}`);
      before = { ...counts };
    },
  };
}

/** One rider's session: look around, plan a trip, change bike, search, and show places along the route. */
test('a planning session stays inside the request budget', async ({ page }) => {
  const { counts, phases, order, idle, phase } = await countRequests(page);
  await page.goto('/planner.html');
  const canvas = page.locator('.maplibregl-canvas');
  await expect(canvas).toBeVisible();
  await idle();
  phase('load');
  // Terrain and overlays wait for the basemap. Every total stays the same when they do not, so only the order shows it.
  const lastBasemap = Math.max(...['basemap', 'glyphs', 'sprites'].map(kind => order.lastIndexOf(kind)));
  const firstDetail = order.findIndex(kind => kind === 'terrain' || kind === 'overlays');
  expect(firstDetail, `load order: ${order.join(' ')}`).toBeGreaterThan(lastBasemap);
  const { x, y, width, height } = await canvas.boundingBox();
  const at = (fx, fy) => [x + width * fx, y + height * fy];

  // Once a route crosses the middle of the map, a press there grabs the route line instead of the map.
  const drag = async (dx, dy, [fx, fy] = [0.5, 0.97]) => {
    const [cx, cy] = at(fx, fy);
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(cx + dx, cy + dy, { steps: 8 });
    await page.mouse.up();
    await idle();
  };
  const zoom = async delta => {
    await page.mouse.move(...at(0.5, 0.5));
    await page.mouse.wheel(0, delta);
    await idle();
  };
  const callout = page.getByRole('dialog', { name: 'Map details' });
  const addPoint = async (fx, fy, choice) => {
    if (await callout.isVisible()) await callout.getByRole('button', { name: 'Close' }).click();
    await page.mouse.click(...at(fx, fy));
    await page.getByRole('button', { name: choice }).click();
    await idle();
  };

  // Zoom in to street level, wander in a loop, and zoom back out.
  for (let i = 0; i < 4; i++) await zoom(-400);
  for (const [dx, dy] of [[-300, 0], [-300, 0], [0, 200], [300, 0], [300, 0], [0, -200], [-300, -100], [300, 100]]) await drag(dx, dy, [0.5, 0.5]);
  for (let i = 0; i < 4; i++) await zoom(400);
  phase('look around');

  await addPoint(0.3, 0.5, /Start here/);
  await addPoint(0.7, 0.5, /Finish here/);
  for (const [fx, fy] of [[0.4, 0.25], [0.55, 0.78], [0.62, 0.22]]) await addPoint(fx, fy, /^Shape$/);
  await addPoint(0.45, 0.65, /^Visit$/);
  phase('plan');

  // A rider drags a marker slowly, so the route previews mid-drag as well as after the release.
  const marker = page.getByRole('button', { name: 'Shaping point' }).nth(1);
  for (const dx of [60, -60]) {
    const { x: mx, y: my, width: mw, height: mh } = await marker.boundingBox();
    await page.mouse.move(mx + mw / 2, my + mh / 2);
    await page.mouse.down();
    await page.mouse.move(mx + mw / 2 + dx / 2, my + mh / 2 + 20, { steps: 4 });
    await page.waitForTimeout(500);
    await page.mouse.move(mx + mw / 2 + dx, my + mh / 2 + 40, { steps: 4 });
    await page.mouse.up();
    await idle();
  }
  phase('drag markers');

  for (const bike of [/Gravel/, /Road/, /Gravel/, /Mountain|MTB/, /Touring/, /Gravel/]) {
    await page.getByRole('combobox', { name: 'Bike' }).click();
    await page.getByRole('option', { name: bike }).click();
    await idle();
  }
  phase('switch bike');

  for (const [dx, dy] of [[-250, -80], [250, 80], [-250, 0], [250, 0]]) await drag(dx, dy);
  await zoom(-300);
  await zoom(300);
  phase('look along the route');

  const search = page.getByLabel('Find a place or ask about the route');
  for (const text of ['bakery', 'campsite', 'hotel']) {
    await search.fill(text);
    await search.press('Enter');
    await idle();
    await search.press('Escape');
  }
  phase('search');

  await page.getByRole('button', { name: 'Map settings' }).click();
  await page.getByText('Places', { exact: true }).click();
  await page.getByRole('button', { name: /^Highlight .* at every zoom$/ }).first().click();
  await idle();
  phase('show places');
  await drag(-200, -60);
  await drag(200, 60);
  phase('pan with places');

  const over = Object.entries(counts).filter(([kind, n]) => n > (budget.limits[kind] ?? 0));
  const cloudflare = CLOUDFLARE.reduce((sum, kind) => sum + (counts[kind] ?? 0), 0);
  console.log(`${phases.join('\n')}\ntotal ${JSON.stringify(counts)} cloudflare=${cloudflare}`);
  expect(over, `categories over their limit in request-budget.json (kind, count): ${JSON.stringify(over)}\n${phases.join('\n')}`).toEqual([]);
});
