// Makes the tile pictures of the map settings from a running development server of the planner:
// node planner/tile-pictures.mjs --base http://127.0.0.1:5316 [--only cycling,relief]
import { mkdirSync, writeFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { chromium } from '@playwright/test';

const { values: args } = parseArgs({ options: { base: { type: 'string' }, only: { type: 'string' } } });
if (!args.base) throw new Error('Give the address of the development server with --base.');

// Central Switzerland. Each route tile shows its own place, so the three tiles tell apart without the colours.
const TILES = {
    cycling: { center: [8.287, 47.068], zoom: 11 },
    hiking: { center: [8.5948, 46.6447], zoom: 11 },
    mtb: { center: [7.936, 46.857], zoom: 11.5 },
    snow: { center: [8.671, 46.668], zoom: 9.5 },
    weather: { center: [8.6002, 46.706], zoom: 11 },
    wind: { center: [8.4017, 46.886], zoom: 8 },
    relief: { center: [8.509, 46.859], zoom: 12 },
    contours: { center: [8.501, 46.767], zoom: 10.5 },
    closures: { center: [8.5815, 46.63465], zoom: 14 },
};
const NAMES = { cycling: 'Cycling', hiking: 'Hiking', mtb: 'MTB', snow: 'Snow', weather: 'Weather', wind: 'Wind', relief: 'Relief', contours: 'Contours', closures: 'Closures' };
const LOOKS = ['light', 'dark'];
// The tile shows 81 × 54 CSS px of the map at 2 device pixels.
const SIZE = { width: 81, height: 54 };
const OUT = new URL('../../../src/components/planner/tiles/', import.meta.url);

const ids = args.only?.split(',') ?? Object.keys(TILES);
const browser = await chromium.launch(process.env.HTTPS_PROXY ? { proxy: { server: process.env.HTTPS_PROXY, bypass: '127.0.0.1,localhost' } } : {});
const pictures = [];
for (const look of LOOKS) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: look });
    // The layer date follows today: the data layers show 15 May.
    await page.clock.setFixedTime(new Date('2026-05-15T12:00:00'));
    await page.goto(`${args.base}/planner.html#/planner`);
    await page.waitForFunction(() => window.plannerMap?.loaded(), null, { timeout: 90_000 });
    await page.addStyleTag({ content: '.maplibregl-ctrl-attrib { display: none !important; }' });
    const box = await page.locator('.map-area').boundingBox();
    const clip = { x: Math.round(box.x + (box.width - SIZE.width) / 2), y: Math.round(box.y + (box.height - SIZE.height) / 2), ...SIZE };
    for (const id of ids) {
        // Each picture has its own layer and the relief; Relief and Contours stand alone.
        const on = id === 'relief' || id === 'contours' ? [id] : [id, 'relief'];
        await page.getByRole('button', { name: 'Map settings', exact: true }).click();
        const dialog = page.getByRole('dialog', { name: 'Map settings' });
        for (const tile of Object.keys(NAMES)) {
            const button = dialog.getByRole('button', { name: NAMES[tile], exact: true });
            if (await button.count() && (await button.getAttribute('aria-pressed') === 'true') !== on.includes(tile)) await button.click();
        }
        await page.keyboard.press('Escape');
        await page.evaluate(view => new Promise(resolve => {
            const map = window.plannerMap;
            map.jumpTo(view);
            map.once('idle', resolve);
            map.triggerRepaint();
        }), TILES[id]);
        // The data layers draw their tiles after the map is idle once.
        await page.waitForTimeout(1500);
        pictures.push([`${id}-${look}.jpg`, await page.screenshot({ type: 'jpeg', quality: 85, clip })]);
        console.log(look, id);
    }
    await page.close();
}
await browser.close();
// Last: a write reloads the page of the development server.
mkdirSync(OUT, { recursive: true });
for (const [name, picture] of pictures) writeFileSync(new URL(name, OUT), picture);
