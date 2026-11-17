// The pictures of the prototype: node shoot.mjs [name,name]; without names, all of them.
import { writeFileSync } from 'node:fs';
import { launch, open, settle, layers, view, snap, OUT } from './lib.mjs';
const only = process.argv[2]?.split(',');
const want = name => !only || only.includes(name);
const ON = ['Cycling', 'Snow', 'Relief', 'Contours', 'Closures'];
const CENTER = [8.60, 46.70], ZOOM = 10.2;
const browser = await launch();
const { page, log } = await open(browser, { form: 'tiles', center: CENTER, zoom: ZOOM });
const form = async name => { await page.evaluate(form => window.__msForm(form), name || null); await page.waitForTimeout(200); };
const toggle = page.getByRole('button', { name: 'Map settings', exact: true });
const dialog = page.getByRole('dialog', { name: 'Map settings' });
const count = text => text.split(/\s+/).filter(word => /[\p{L}\p{N}]/u.test(word)).length;
const words = {};
const map = name => snap(page.locator('.map-area'), { path: `${OUT}/${name}.png` });
/** The map with the popup open and the given layers on. */
async function state(name, on) {
    await form(name);
    await layers(page, name, on);
    await dialog.evaluate(panel => { panel.scrollTop = 0; });
    await page.mouse.move(300, 500);
    await settle(page, 1200);
}

for (const [name, label] of [['', 'today'], ['tiles', 'tiles'], ['list', 'list']]) {
    if (want(`${label}-open`)) {
        await state(name, []);
        await map(`${label}-open`);
        words[`${label}-open`] = count(await dialog.innerText());
    }
    if (want(`${label}-on`) || (label === 'tiles' && (want('tiles-legend-info') || want('tiles-button-on')))) {
        await state(name, ON);
        if (want(`${label}-on`)) await map(`${label}-on`);
        words[`${label}-on`] = count(await dialog.innerText());
        if (label === 'tiles') {
            words['tiles-on, legend on the map'] = count(await page.locator('.map-legend').innerText());
            // A click on the map side closes the popup, as a click outside does.
            const about = page.getByRole('button', { name: 'About these layers' });
            await about.click();
            await page.mouse.move(300, 300);
            await page.waitForTimeout(300);
            if (want('tiles-legend-info')) await map('tiles-legend-info');
            words['tiles-on, legend with its info open'] = count(await page.locator('.map-legend').innerText());
            await about.click();
            if (await toggle.getAttribute('aria-expanded') === 'true') await toggle.click();
            await page.mouse.move(300, 300);
            await page.waitForTimeout(300);
            const box = await page.locator('.map-area').boundingBox();
            if (want('tiles-button-on')) await snap(page, { path: `${OUT}/tiles-button-on.png`, clip: { x: box.x + box.width - 250, y: box.y, width: 250, height: 290 } });
        }
    }
    if (label === 'today') {
        await state(name, ['Cycling', 'Relief', 'Contours', 'Closures']);
        words['today, as a new rider sees it'] = count(await dialog.innerText());
    }
}
for (const name of ['tiles', 'list']) {
    if (!want(`${name}-dark`)) continue;
    await page.emulateMedia({ colorScheme: 'dark' });
    await state(name, ON);
    await map(`${name}-dark`);
}
await page.emulateMedia({ colorScheme: 'light' });
for (const name of ['tiles', 'list']) {
    if (!want(`${name}-narrow`)) continue;
    await page.setViewportSize({ width: 390, height: 844 });
    await state(name, ON);
    await page.locator('.map-area').evaluate(element => element.scrollIntoView({ block: 'end' }));
    await view(page, [8.60, 46.66], 9.6);
    await settle(page, 1200);
    await snap(page, { path: `${OUT}/${name}-narrow.png` });
}
console.log(JSON.stringify(words, null, 1));
if (!only) writeFileSync(`${OUT}/words.json`, JSON.stringify(words, null, 1));
console.log(log.slice(0, 5).join('\n'));
await browser.close();
