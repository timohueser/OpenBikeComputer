import { chromium } from '/home/user/wt-mapsettings/builder/web/tests/browser/node_modules/playwright/index.mjs';
export const OUT = '/tmp/claude-0/-home-user-OpenBikeComputer/2d1ce3ff-4225-5a70-a539-37426ae550b6/scratchpad/mapsettings';
export const BASE = 'http://127.0.0.1:5314/planner.html';
export const launch = () => chromium.launch({ proxy: { server: process.env.HTTPS_PROXY, bypass: '127.0.0.1,localhost' } });
/** A planner page with no route at the given view. */
export async function open(browser, { form = '', scheme = 'light', viewport = { width: 1270, height: 873 }, center = [8.60, 46.70], zoom = 10.2, extra = '' } = {}) {
    const context = await browser.newContext({ viewport, deviceScaleFactor: 2, colorScheme: scheme });
    const page = await context.newPage();
    const log = [];
    page.on('console', m => { if (m.type() === 'error') log.push(m.text().slice(0, 300)); });
    page.on('pageerror', e => log.push(`pageerror: ${String(e).slice(0, 300)}`));
    await page.goto(`${BASE}?${form ? `mapsettings=${form}` : ''}${extra}#/planner`);
    await page.waitForFunction(() => window.__ms?.map?.loaded(), null, { timeout: 90000 });
    await page.addStyleTag({ content: '.maplibregl-ctrl-attrib.maplibregl-compact-show { display: none !important; }' });
    if (center) await view(page, center, zoom);
    return { page, context, log };
}
export async function settle(page, ms = 500) {
    for (let quiet = 0; quiet < 2;) {
        await page.evaluate(() => new Promise(resolve => {
            const { map } = window.__ms;
            const check = () => (map.loaded() && !map.isMoving() ? resolve() : setTimeout(check, 100));
            map.triggerRepaint(); setTimeout(check, 150);
        }));
        await page.waitForTimeout(ms);
        quiet = await page.evaluate(() => window.__ms.map.loaded()) ? quiet + 1 : 0;
    }
}
export async function view(page, center, zoom) {
    await page.evaluate(([center, zoom]) => window.__ms.map.jumpTo({ center, zoom }), [center, zoom]);
    await settle(page);
}
export const shot = (page, name, selector = '.map-area') => snap(page.locator(selector), { path: `${OUT}/${name}.png` });

const NAMES = ['Cycling', 'Hiking', 'MTB', 'Snow', 'Weather', 'Wind', 'Relief', 'Contours', 'Closures'];
/** Opens the popup and turns on exactly the layers in `on`; the popup stays open. */
export async function layers(page, form, on) {
    const toggle = page.getByRole('button', { name: 'Map settings', exact: true });
    if (await toggle.getAttribute('aria-expanded') !== 'true') await toggle.click();
    const dialog = page.getByRole('dialog', { name: 'Map settings' });
    if (form) {
        const [role, state] = form === 'tiles' ? ['button', 'aria-pressed'] : ['switch', 'aria-checked'];
        // Twice: a layer of a one-at-a-time row turns its neighbour off.
        for (let pass = 0; pass < 2; pass++) for (const name of NAMES) {
            const control = dialog.getByRole(role, { name, exact: true });
            if ((await control.getAttribute(state) === 'true') !== on.includes(name)) await control.click();
        }
    } else {
        await dialog.getByRole('radiogroup', { name: 'Route network' }).getByRole('radio', { name: ['Cycling', 'Hiking', 'MTB'].find(n => on.includes(n)) ?? 'Off' }).click();
        await dialog.getByRole('radiogroup', { name: 'Data layer' }).getByRole('radio', { name: ['Snow', 'Weather', 'Wind'].find(n => on.includes(n)) ?? 'Off' }).click();
        await dialog.getByRole('checkbox', { name: 'Closures & access' }).setChecked(on.includes('Closures'));
        const terrain = dialog.locator('details', { hasText: 'Terrain' });
        await terrain.locator('summary').click();
        await dialog.getByRole('checkbox', { name: 'Relief' }).setChecked(on.includes('Relief'));
        await dialog.getByRole('checkbox', { name: 'Contours' }).setChecked(on.includes('Contours'));
        await terrain.locator('summary').click();
    }
    await settle(page, 900);
}

/** A screenshot that tries again when the page does not answer in time. */
export async function snap(target, options) {
    for (let attempt = 0; ; attempt++) {
        try { return await target.screenshot({ timeout: 15000, ...options }); }
        catch (error) { if (attempt === 3) throw error; console.log('screenshot again'); }
    }
}
