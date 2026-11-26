import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const drawn = (id, kind, lon, from, to, extra = {}) => ({
  id, kind, label: id, coordinate: [lon, 47], ...extra,
  ...kind !== 'start' && { leg: 'drawn', drawn: [[lon - 0.05, 47, from], [lon - 0.025, 47, (from + to) / 2], [lon, 47, to]] },
});

// A plan of two days with drawn legs, so its line needs no routing request: 15.2 km along the parallel 47, the night in
// the middle.
const plan = {
  id: 'two', name: 'Two days', updatedAt: 0, summary: null,
  trip: {
    mode: 'trip', bike: 'touring', days: 2, daysSet: true, routeOrder: ['up', 'night-1', 'flat'], points: [
      drawn('start', 'start', 8, 400, 400), drawn('up', 'waypoint', 8.05, 400, 600), drawn('night-1', 'night', 8.1, 600, 600, { night: 1, label: 'Inn' }),
      drawn('flat', 'waypoint', 8.15, 600, 600), drawn('finish', 'finish', 8.2, 600, 400),
    ],
  },
};
const CAFE = { name: 'Linde', kind: 'cafe', lon: 8.12, lat: 47.001, precision: 'place' };
/** Fountains at km 2, 5, 9 and 13 of the plan. */
const FOUNTAINS = [2, 5, 9, 13].map(km => ({ source: `n/${km}`, name: `Brunnen ${km}`, kind: 'drinking_water', lon: 8 + km / 75.9, lat: 47.001,
  precision: 'place', distance: 0.1, position: { along: km, distance: 0.1 } }));

/** Opens the plan of two days. The search answers each request with `answer`, and the map fits a part at once, so it is
 * at rest when a drag ends. */
async function openPlan(page, answer) {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    if (request.url().endsWith('/query')) return route.fulfill({ ...CORS_PREFLIGHT, status: 200, json: answer(request.postDataJSON()) });
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async record => {
    const { library } = window.coverTest;
    await library.saveRoute(record);
    await library.setState('active', record.id);
  }, plan);
  await page.goto('/planner.html#/planner');
}
/** Drags across `box` from the share `from` of its width to `to`. */
async function drag(page, box, from, to) {
  const { x, y, width, height } = await box.boundingBox();
  await page.mouse.move(x + width * from, y + height / 2);
  await page.mouse.down();
  await page.mouse.move(x + width * (from + to) / 2, y + height / 2);
  await page.mouse.move(x + width * to, y + height / 2);
  await page.mouse.up();
}

test('the planner profile: a drag selects a part that is the search area, a press on a day-end handle selects none, and Clear ends it', async ({ page }) => {
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  const areas = [];
  await openPlan(page, input => {
    areas.push(input.request?.where);
    return { type: 'places', request: { type: 'places', what: ['cafe'] }, results: [CAFE] };
  });

  const profile = page.locator('.elevation');
  const plot = profile.locator('.plot');
  await expect(plot).toBeVisible();
  // The map reports its view once when it has loaded; that report would count as a move after the search.
  await expect(page.getByLabel('Route map')).toHaveAttribute('aria-busy', 'false', { timeout: 30_000 });
  await page.getByRole('combobox', { name: 'Find a place or ask about the route' }).fill('cafes');
  await page.keyboard.press('Enter');
  await expect(page.locator('.results')).toContainText('Linde');

  const part = profile.locator('.title .part');
  await profile.getByRole('radio', { name: 'Whole route' }).click();
  await drag(page, plot, 0.4, 0.6);
  // The ends of the stretch go out to a tenth of a km.
  await expect(part).toContainText(/^km\s6\.0 to km\s9\.2 · 3\.2\skm · \d+\sm up/);
  await expect(profile.locator('.pale')).toHaveCount(2);
  // The part is the stretch of "Where", so the list of places along the route starts there.
  await expect(page.locator('[data-slot="where"] strong')).toHaveText('6–9.2 km');
  await expect.poll(() => areas.at(-1)).toEqual({ along: { ref: 'km', from: { value: 6, unit: 'km' }, to: { value: 9.2, unit: 'km' } } });
  // The fit of the map to the part is no move of the map by the rider.
  const searchView = page.getByRole('button', { name: 'Search this map view' });
  await page.waitForTimeout(500);
  await expect(searchView).toHaveCount(0);
  await part.getByRole('button', { name: 'Clear' }).click();
  await expect(part).toHaveCount(0);

  // A move of the map by the rider offers the search of the new view.
  const map = await page.locator('.maplibregl-canvas').boundingBox();
  await page.mouse.move(map.x + map.width / 2, map.y + map.height / 3);
  await page.mouse.down();
  await page.mouse.move(map.x + map.width / 2 + 80, map.y + map.height / 3, { steps: 4 });
  await page.mouse.up();
  await expect(searchView).toBeVisible();

  // A drag that starts on the handle of the night selects no part.
  const handle = await profile.locator('.handle').boundingBox();
  await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
  await page.mouse.down();
  await page.mouse.move(handle.x + handle.width / 2 + 30, handle.y + handle.height / 2);
  await page.mouse.move(handle.x + handle.width / 2 + 60, handle.y + handle.height / 2);
  await page.mouse.up();
  await expect(part).toHaveCount(0);
  await expect(profile.locator('.pale')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('"Where" on the profile: the places of "Find" are dots, a drag sets a stretch that the list and the main profile show, and a day button sets a day', async ({ page }) => {
  const errors = [], areas = [];
  page.on('pageerror', error => errors.push(error.message));
  await openPlan(page, input => {
    const request = input.request ?? { type: 'places', what: ['drinking_water'] }, along = request.where?.along;
    if (input.limit !== 100) areas.push(request.where);
    return { type: 'places', request, results: FOUNTAINS.filter(({ position }) => !along || (position.along >= along.from.value && position.along <= along.to.value)) };
  });
  await expect(page.getByLabel('Route map')).toHaveAttribute('aria-busy', 'false', { timeout: 30_000 });
  await page.getByRole('combobox', { name: 'Find a place or ask about the route' }).fill('drinking water');
  await page.keyboard.press('Enter');
  const results = page.locator('.results');
  await expect(results).toContainText('Brunnen 13');
  await page.locator('[data-slot="where"]').click();
  const panel = page.locator('#search-panel'), plot = panel.locator('.plot'), where = page.locator('[data-slot="where"] strong');
  const dots = () => panel.locator('.dot').evaluateAll(all => all.map(dot => dot.classList.contains('in')));
  await expect.poll(dots).toEqual([true, true, true, true]);
  // The pointer names the place under it.
  await panel.locator('.dot').first().hover();
  await expect.poll(() => panel.locator('.dot').first().evaluate(dot => getComputedStyle(dot, '::after').content)).toContain('Brunnen 2');
  // A vertical swipe scrolls the panel; a horizontal drag sets the stretch.
  await expect(plot).toHaveCSS('touch-action', 'pan-y');

  await drag(page, plot, 0.25, 0.7);
  await expect(where).toHaveText(/^\d+\.\d–\d+\.\d km$/);
  const [from, to] = (await where.textContent()).match(/[\d.]+/g).map(Number);
  expect(from).toBeCloseTo(3.8, 0);
  expect(to).toBeCloseTo(10.6, 0);
  await expect.poll(dots).toEqual([false, true, true, false]);
  await expect.poll(() => areas.at(-1)).toEqual({ along: { ref: 'km', from: { value: from, unit: 'km' }, to: { value: to, unit: 'km' } } });
  await expect(results).not.toContainText('Brunnen 13');
  await expect(results).toContainText('Brunnen 9');
  await expect(page.locator('.elevation .title .part')).toContainText(new RegExp(`^km\\s${from} to km\\s${to} `));
  // The handles are range inputs: an arrow key moves an end by 1 % of the route, and End takes it to the finish.
  const end = panel.getByRole('slider', { name: 'End of the stretch' });
  await end.press('ArrowLeft');
  await expect(where).toHaveText(`${from}–${(Math.ceil((to - 0.152) * 10) / 10).toFixed(1)} km`);
  await end.press('End');
  await expect(where).toHaveText(`${from}–15.2 km`);
  // The handle sits on the end of the stretch, at the finish.
  expect(await end.evaluate(input => input.valueAsNumber / Number(input.max))).toBeCloseTo(1, 6);

  await panel.getByRole('button', { name: 'Day 2' }).click();
  await expect(where).toHaveText('Day 2');
  await expect(panel.getByRole('button', { name: 'Day 2' })).toHaveAttribute('aria-pressed', 'true');
  await expect.poll(dots).toEqual([false, false, true, true]);
  await panel.getByRole('radio', { name: 'Middle' }).click();
  await expect(where).toHaveText('Middle of Day 2');
  expect(errors).toEqual([]);
});
