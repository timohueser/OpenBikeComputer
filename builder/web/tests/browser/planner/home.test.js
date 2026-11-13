import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

const HOME_VIEWS = 'https://maps.openbikecomputer.com/home-views';
const NOON = new Date(2026, 10, 13, 12);
const PIXEL = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64');
const view = id => ({ id, name: `View ${id}`, region: 'Central Switzerland', lat: 46.8, lon: 8.4, sunrise: '07:00', sunset: '17:00', night: false });
const manifest = { format: 1, set: '0123456789abcdef', date: '2026-10-15', width: 2560, height: 1440, credits: ['© Test data'], views: ['a', 'b', 'c'].map(view) };

/** Answers the hosts of the planner build; `homeViews` answers the home views, else they fail. */
async function start(page, homeViews) {
  await page.clock.setFixedTime(NOON);
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    if (homeViews && request.url().startsWith(HOME_VIEWS)) return homeViews(route, request.url().slice(HOME_VIEWS.length));
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
}

test('a first visit with no home views shows the slogan, the previews and the drawn hills, also for an unknown page', async ({ page }) => {
  await start(page);
  for (const path of ['/planner.html', '/planner.html#/nowhere']) {
    await page.goto(path);
    await expect(page.getByRole('heading', { name: '[Slogan goes here]' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'What you can do here' }).getByRole('listitem')).toHaveCount(4);
    await expect(page.locator('svg.hills')).toBeVisible();
    await expect(page.getByRole('complementary', { name: 'You look at' })).toHaveCount(0);
  }
});

test('the view of the day shows by the rule with its credits; a missing image reads the manifest again, then shows the hills', async ({ page }) => {
  const manifests = [];
  await start(page, (route, path) => {
    if (path === '/manifest.json') {
      manifests.push(path);
      return route.fulfill({ json: manifest, headers: { 'access-control-allow-origin': '*' } });
    }
    return path.includes('/a/') ? route.fulfill({ status: 404 }) : route.fulfill({ body: PIXEL, contentType: 'image/webp' });
  });
  await page.goto('/planner.html?cover=none');
  await page.waitForFunction(() => window.coverTest);
  await page.evaluate(async start => {
    const summary = { line: [8, 47, 8.1, 47.1], heights: [400, 500], bounds: [8, 47, 8.1, 47.1], lengthM: 62_000, climbM: 400, highestM: 500, seconds: 9000, bike: 'gravel', start, movingS: 9000 };
    await window.coverTest.library.saveRide({ key: '20261109T090000Z', name: 'Kaiserstuhl', source: 'file', format: 'gpx', maxHr: null, ftp: null, summary });
  }, new Date(2026, 10, 9, 10).getTime() / 1000);

  await page.goto('/planner.html#/');
  await expect(page.getByText('This year, by bike')).toBeVisible();
  await expect(page.getByText('Your last ride was Kaiserstuhl, 4 days ago.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Kaiserstuhl' })).toHaveAttribute('href', '#/ride/20261109T090000Z');

  // 13 Nov 2026 is day 20,770 since 1970: view 20,770 mod 3 = 1, at noon.
  const box = page.getByRole('complementary', { name: 'You look at' });
  await expect(box.getByRole('heading', { name: 'View b' })).toBeVisible();
  await expect(page.locator('img.backdrop')).toHaveAttribute('src', `${HOME_VIEWS}/0123456789abcdef/b/noon.webp`);
  await box.getByText('Data credits').click();
  await expect(box.getByText('© Test data')).toBeVisible();
  expect(manifests).toHaveLength(1);

  await box.getByRole('button', { name: 'Next view' }).click();
  await expect(page.locator('img.backdrop')).toHaveAttribute('src', `${HOME_VIEWS}/0123456789abcdef/c/noon.webp`);
  // The image of view a fails, and the manifest names the same set again.
  await box.getByRole('button', { name: 'Next view' }).click();
  await expect(box).toHaveCount(0);
  await expect(page.locator('svg.hills')).toBeVisible();
  expect(manifests).toHaveLength(2);
});
