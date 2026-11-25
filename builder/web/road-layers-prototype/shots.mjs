// Screenshots of the road layers from a running dev server: node road-layers-prototype/shots.mjs http://localhost:5173
import { createRequire } from 'node:module';
const { chromium } = createRequire(import.meta.url)('playwright');
const base = process.argv[2] ?? 'http://localhost:5173';
const out = new URL('./', import.meta.url).pathname;
// Kriens and the Sonnenberg, south-west of Lucerne: flat town, hills and the Pilatus foothills. The settings panel covers the right.
const views = { near: { center: [8.28, 47.04], zoom: 14, padding: { right: 420 } }, far: { center: [8.29, 47.02], zoom: 12, padding: { right: 420 } },
    // Pilatus, the steepest ground of the test area.
    mountain: { center: [8.255, 46.98], zoom: 13, padding: { right: 420 } } };
const tiles = ['Steepness', 'Surface', 'Trail grade', 'Slope angle'];
const shots = [
    ...['Steepness', 'Surface', 'Trail grade'].flatMap(layer => Object.keys(views).map(view => ({ layer, view, look: 'light' }))),
    { layer: 'Slope angle', view: 'mountain', look: 'light' },
    { layer: 'Steepness', view: 'near', look: 'dark' },
];
const browser = await chromium.launch(process.env.HTTPS_PROXY ? { proxy: { server: process.env.HTTPS_PROXY, bypass: '127.0.0.1,localhost' } } : {});
for (const look of ['light', 'dark']) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: look });
    page.on('pageerror', error => console.log('page error', error.message));
    page.on('crash', () => console.log('crash'));
    page.on('framenavigated', frame => frame === page.mainFrame() && console.log('nav', frame.url()));
    page.on('console', m => m.type() === 'error' && console.log('console', m.text().slice(0, 300)));
    await page.goto(`${base}/planner.html#/planner`);
    await page.waitForFunction(() => window.plannerMap?.loaded(), null, { timeout: 90_000 });
    await page.waitForTimeout(8000);
    await page.waitForFunction(() => window.plannerMap?.loaded(), null, { timeout: 90_000 });
    for (const shot of shots.filter(shot => shot.look === look)) {
        await page.evaluate(view => window.plannerMap.jumpTo(view), views[shot.view]);
        const dialog = page.getByRole('dialog', { name: 'Map settings' });
        if (!await dialog.isVisible()) await page.getByRole('button', { name: 'Map settings', exact: true }).click();
        for (const tile of tiles) {
            const button = dialog.getByRole('button', { name: tile, exact: true });
            if ((await button.getAttribute('aria-pressed') === 'true') !== (tile === shot.layer)) await button.click();
        }
        await page.evaluate(() => new Promise(resolve => { const map = window.plannerMap; map.once('idle', resolve); map.triggerRepaint(); }));
        await page.waitForTimeout(1500);
        const name = `${shot.layer.toLowerCase().replace(' ', '-')}-${shot.view}-${look}.jpg`;
        await page.screenshot({ path: out + name, type: 'jpeg', quality: 82 });
        console.log(name);
    }
    await page.close();
}
await browser.close();
