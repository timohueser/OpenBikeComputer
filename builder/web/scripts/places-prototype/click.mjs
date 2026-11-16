import { AREAS, launch, open, settle } from './lib.mjs';
const browser = await launch();
for (const mode of ['groups', 'badges']) {
    const { page, context, log } = await open(browser, { mode, center: AREAS.valley, zoom: 13, scale: 1 });
    const pick = cluster => page.evaluate(cluster => { const item = window.__places.drawn().filter(i => (i.count > 1) === cluster && i.x > 50 && i.x < 800 && i.y > 50 && i.y < 560)[0]; return item; }, cluster);
    const box = await page.locator('.map-area canvas.maplibregl-canvas').boundingBox();
    const group = await pick(true);
    await page.mouse.click(box.x + group.x, box.y + group.y);
    await page.waitForTimeout(1500); await settle(page);
    console.log(mode, 'click on', group.kinds, 'x' + group.count, '-> zoom', await page.evaluate(() => window.__places.map.getZoom().toFixed(2)));
    const single = await pick(false);
    if (single) {
        await page.mouse.click(box.x + single.x, box.y + single.y);
        await page.waitForTimeout(1500);
        console.log(mode, 'click on a single', single.kinds, '-> popup:', (await page.locator('.maplibregl-popup, [class*=place-card], [class*=popup]').first().innerText().catch(() => 'none')).replace(/\s+/g, ' ').slice(0, 80));
    }
    if (log.some(l => !l.startsWith('places prototype'))) console.log(log.filter(l => !l.startsWith('places prototype')).slice(0, 3));
    await context.close();
}
await browser.close();
