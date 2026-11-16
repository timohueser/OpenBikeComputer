import { chromium } from '/home/user/wt-places/builder/web/tests/browser/node_modules/playwright/index.mjs';
export const OUT = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/places';
export const BASE = 'http://localhost:5303/planner.html';
export const AREAS = { town: [8.306, 47.052], valley: [8.57, 46.62], mountains: [8.50, 46.65] };
export const launch = () => chromium.launch({ proxy: { server: process.env.HTTPS_PROXY, bypass: '127.0.0.1,localhost' } });
/** A planner page with no route, the map 900 x 650 CSS px, at the given view. */
export async function open(browser, { mode, kinds = 'default', center, zoom, extra = '', scale = 2 }) {
    const context = await browser.newContext({ viewport: { width: 1270, height: 873 }, deviceScaleFactor: scale, colorScheme: 'light' });
    const page = await context.newPage();
    const log = [];
    page.on('console', m => { if (m.type() === 'error' || m.text().startsWith('places prototype')) log.push(m.text().slice(0, 300)); });
    page.on('pageerror', e => log.push(`pageerror: ${String(e).slice(0, 300)}`));
    await page.goto(`${BASE}?places=${mode}&kinds=${kinds}${extra}#/planner`);
    await page.waitForFunction(() => window.__places?.map?.loaded(), null, { timeout: 90000 });
    await page.addStyleTag({ content: '.maplibregl-ctrl-attrib.maplibregl-compact-show { display: none !important; }' });
    if (center) await view(page, center, zoom);
    return { page, context, log };
}
export async function settle(page) {
    for (let quiet = 0; quiet < 2;) {
        await page.evaluate(() => new Promise(resolve => {
            const { map } = window.__places;
            const check = () => (map.loaded() && !map.isMoving() && !window.__places.busy ? resolve() : setTimeout(check, 100));
            map.triggerRepaint(); setTimeout(check, 150);
        }));
        await page.waitForLoadState('networkidle').catch(() => {});
        await page.waitForTimeout(350);
        quiet = await page.evaluate(() => window.__places.map.loaded() && !window.__places.busy) ? quiet + 1 : 0;
    }
}
export async function view(page, center, zoom) {
    await page.evaluate(([center, zoom]) => window.__places.map.jumpTo({ center, zoom }), [center, zoom]);
    await settle(page);
}
export const shot = (page, name) => page.locator('.map-area').screenshot({ path: `${OUT}/${name}.png` });
