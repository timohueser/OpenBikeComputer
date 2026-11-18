import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

/** The marks of the place tile of `stubs.js`: a fountain alone, and two restaurants on one spot. */
test('a mark opens the popup of its place, or a list of its places, under the pins and the popup', async ({ page }) => {
  const tiles = [];
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
    if (/\/places\/\d+\/\d+\/\d+\.mvt$/.test(request.url())) tiles.push(new URL(request.url()).pathname);
    if (request.method() === 'OPTIONS') return route.fulfill(CORS_PREFLIGHT);
    const response = respond(request);
    return response ? route.fulfill(response) : route.abort();
  });
  await page.goto('/planner.html#/planner');
  // The quiet icons show from zoom 16; the map starts at zoom 7.
  for (let i = 0; i < 9; i++) {
    await page.getByRole('button', { name: 'Zoom in' }).click();
    await page.waitForTimeout(500);
  }
  const marks = page.getByRole('group', { name: 'Places on the map' });
  const popup = page.getByRole('dialog', { name: 'Map details' });
  const fountain = marks.getByRole('button', { name: 'Town fountain, Fountain' });
  const map = await page.locator('.map-canvas').boundingBox();
  await expect(fountain).toBeVisible();

  // Each place tile loads once in a session. The view loads 11/1075/705 and 11/1075/706. Pans inside them load nothing,
  // and a pan of 3,600 px west loads the two tiles of column 1074 and leaves column 1075; the pan back loads nothing. The
  // count is exact on this build: with no cache in `placeTile`, the pan back loads column 1075 again.
  expect(tiles).toHaveLength(2);
  const drag = async dx => {
    const [x, y] = [map.x + map.width * (dx > 0 ? 0.1 : 0.9), map.y + map.height * 0.8];
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x + dx, y, { steps: 10 });
    // A still pointer before the release leaves no inertia, so the pans add up.
    await page.waitForTimeout(300);
    await page.mouse.up();
    await page.waitForTimeout(800);
  };
  for (const dx of [120, -120, 120, -120, 600, 600, 600, 600, 600, 600, -600, -600, -600, -600, -600, -600]) await drag(dx);
  expect(tiles.sort()).toEqual(['1074/705', '1074/706', '1075/705', '1075/706'].map(tile => `/places/11/${tile}.mvt`));

  // The keyboard reaches only the marks inside the map.
  await page.locator('.maplibregl-canvas').focus();
  await page.keyboard.press('Tab');
  for (let i = 0; i < 4; i++) {
    const focused = await page.evaluate(() => { const { x, y, width, height } = document.activeElement.getBoundingClientRect(); return { label: document.activeElement.getAttribute('aria-label'), x: x + width / 2, y: y + height / 2 }; });
    expect(focused.x > map.x && focused.x < map.x + map.width && focused.y > map.y && focused.y < map.y + map.height, focused.label).toBe(true);
    await page.keyboard.press('ArrowDown');
  }

  await fountain.click();
  await expect(popup).toContainText('Town fountain');
  const order = await marks.evaluate(root => ({
    beforePin: !!(root.compareDocumentPosition(document.querySelector('.planner-map-pin')) & Node.DOCUMENT_POSITION_FOLLOWING),
    beforePopup: !!(root.compareDocumentPosition(document.querySelector('.maplibregl-popup')) & Node.DOCUMENT_POSITION_FOLLOWING),
    isolation: getComputedStyle(root).isolation,
  }));
  expect(order).toEqual({ beforePin: true, beforePopup: true, isolation: 'isolate' });

  const list = page.getByRole('dialog', { name: 'Places here' });
  // The mark of the open place takes the popup back from the list. The list covers the mark, so the keyboard opens it.
  await marks.getByRole('button', { name: '2 places, Eating' }).click();
  await expect(list).toBeVisible();
  await fountain.focus();
  await page.keyboard.press('Enter');
  await expect(popup).toContainText('Town fountain');
  await expect(list).toHaveCount(0);

  await marks.getByRole('button', { name: '2 places, Eating' }).click();
  await expect(list.getByRole('listitem')).toHaveCount(2);
  await list.getByRole('button', { name: /^Pizzeria/ }).click();
  await expect(popup).toContainText('Pizzeria');
});
