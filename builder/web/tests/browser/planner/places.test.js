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
  const pans = async moves => { for (const dx of moves) await drag(dx); };
  const paths = names => names.map(tile => `/places/11/${tile}.mvt`);
  await pans([120, -120, 120, -120]);
  expect(tiles).toHaveLength(2);
  await pans([600, 600, 600, 600, 600, 600]);
  expect(tiles.slice(2).sort()).toEqual(paths(['1074/705', '1074/706']));
  await pans([-600, -600, -600, -600, -600, -600]);
  expect(tiles.sort()).toEqual(paths(['1074/705', '1074/706', '1075/705', '1075/706']));

  // The keyboard reaches the marks inside the map, north to south and west to east, and not the café in the margin.
  const restaurants = marks.getByRole('button', { name: '2 places, Eating' });
  const tap = marks.getByRole('button', { name: 'Square tap, Water tap' });
  await page.locator('.maplibregl-canvas').focus();
  await page.keyboard.press('Tab');
  await expect(tap).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(restaurants).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(fountain).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await expect(fountain).toBeFocused();
  const centre = async locator => { const { x, y, width, height } = await locator.boundingBox(); return [x + width / 2, y + height / 2]; };
  const over = ([x, y], selector) => page.evaluate(([x, y, selector]) => !!document.elementFromPoint(x, y)?.closest(selector), [x, y, selector]);

  await fountain.click();
  await expect(popup).toContainText('Town fountain');
  const order = await marks.evaluate(root => ({
    beforePin: !!(root.compareDocumentPosition(document.querySelector('.planner-map-pin')) & Node.DOCUMENT_POSITION_FOLLOWING),
    beforePopup: !!(root.compareDocumentPosition(document.querySelector('.maplibregl-popup')) & Node.DOCUMENT_POSITION_FOLLOWING),
    isolation: getComputedStyle(root).isolation,
  }));
  expect(order).toEqual({ beforePin: true, beforePopup: true, isolation: 'isolate' });
  // The pin of the chosen place draws over its mark.
  expect(await over(await centre(fountain), '.planner-map-pin')).toBe(true);

  const list = page.getByRole('dialog', { name: 'Places here' });
  // The list draws over the tap. The mark of the open place takes the popup back from the list; its pin covers it, so the
  // keyboard opens it.
  await restaurants.click();
  await expect(list).toBeVisible();
  expect(await over(await centre(tap), '.maplibregl-popup')).toBe(true);
  await fountain.focus();
  await page.keyboard.press('Enter');
  await expect(popup).toContainText('Town fountain');
  await expect(list).toHaveCount(0);

  // Escape gives the focus back to the opener of the list, also when the popup of the place comes back.
  await restaurants.click();
  await expect(list.getByRole('button', { name: /^Bistro/ })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(popup).toContainText('Town fountain');
  await expect(restaurants).toBeFocused();

  await restaurants.click();
  await expect(list.getByRole('listitem')).toHaveCount(2);
  await list.getByRole('button', { name: /^Pizzeria/ }).click();
  await expect(popup).toContainText('Pizzeria');

  // The control of the kinds chooses the marks of the map.
  await page.getByRole('button', { name: 'Kinds of places' }).click();
  const kinds = page.getByRole('dialog', { name: 'Kinds of places' });
  await kinds.getByRole('checkbox', { name: 'Eating' }).uncheck();
  await expect(restaurants).toHaveCount(0);
  await kinds.getByRole('checkbox', { name: 'Eating' }).check();
  await expect(restaurants).toBeVisible();
  await expect(marks.locator('.disc')).toHaveCount(0);
  await kinds.getByRole('button', { name: 'Highlight Water' }).click();
  await expect(marks.locator('.disc')).not.toHaveCount(0);
  await expect(fountain).toHaveClass(/disc/);
});
