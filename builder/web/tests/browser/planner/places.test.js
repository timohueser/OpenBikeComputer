import { expect, test } from '@playwright/test';
import { CORS_PREFLIGHT, respond } from './stubs.js';

/** The marks of the place tile of `stubs.js`: a fountain alone, and two restaurants on one spot. */
test('a mark opens the popup of its place, or a list of its places, under the pins and the popup', async ({ page }) => {
  await page.route(url => url.hostname !== '127.0.0.1', route => {
    const request = route.request();
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

  await marks.getByRole('button', { name: 'Town fountain, Fountain' }).click();
  await expect(popup).toContainText('Town fountain');
  const order = await marks.evaluate(root => ({
    beforePin: !!(root.compareDocumentPosition(document.querySelector('.planner-map-pin')) & Node.DOCUMENT_POSITION_FOLLOWING),
    beforePopup: !!(root.compareDocumentPosition(document.querySelector('.maplibregl-popup')) & Node.DOCUMENT_POSITION_FOLLOWING),
    isolation: getComputedStyle(root).isolation,
  }));
  expect(order).toEqual({ beforePin: true, beforePopup: true, isolation: 'isolate' });

  await marks.getByRole('button', { name: '2 places, Eating' }).click();
  const list = page.getByRole('dialog', { name: 'Places here' });
  await expect(list.getByRole('listitem')).toHaveCount(2);
  await list.getByRole('button', { name: /^Pizzeria/ }).click();
  await expect(popup).toContainText('Pizzeria');
});
