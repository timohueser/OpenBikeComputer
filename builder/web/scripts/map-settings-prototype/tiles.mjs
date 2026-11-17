// The tile pictures: crops of the planner map at its real scale with one layer on, in both themes.
// node tiles.mjs [name,name] writes out/NAME-THEME.jpg; copy them to builder/web/src/components/planner/tiles/ afterwards (a write there reloads the page).
import { launch, open, settle, layers, view, snap } from './lib.mjs';
const OUT = new URL('./out', import.meta.url).pathname;
const TILES = {
    Cycling: [[8.5948, 46.6447], 11], Hiking: [[8.5948, 46.6447], 11], MTB: [[8.5948, 46.6447], 11],
    // The region limits the view at zoom 8, so the wind crop is cut from beside the centre: PIL crop (320, 98, 482, 206) of the context picture.
    Snow: [[8.671, 46.668], 9.5], Weather: [[8.6002, 46.706], 11], Wind: [[8.4017, 46.886], 8],
    Relief: [[8.501, 46.767], 10.5], Contours: [[8.501, 46.767], 10.5], Closures: [[8.5815, 46.63465], 14],
};
// The region limits the view at zoom 8: the wind tile is cut from beside the centre of its context picture, PIL crop (320, 98, 482, 206).
const only = process.argv[2]?.split(',') ?? Object.keys(TILES);
const overrides = JSON.parse(process.argv[3] ?? '{}');
const browser = await launch();
const { page, log } = await open(browser, { form: 'tiles' });
const box = await page.locator('.map-area').boundingBox();
const around = (w, h) => ({ x: Math.round(box.x + box.width / 2 - w / 2), y: Math.round(box.y + box.height / 2 - h / 2), width: w, height: h });
for (const theme of ['light', 'dark']) {
    await page.emulateMedia({ colorScheme: theme });
    for (const name of only) {
        const [center, zoom] = overrides[name] ?? TILES[name];
        await layers(page, 'tiles', name === 'Relief' || name === 'Contours' ? [name] : [name, 'Relief']);
        await page.keyboard.press('Escape');
        await view(page, center, zoom);
        await settle(page, 1200);
        await snap(page, { path: `${OUT}/${name.toLowerCase()}-${theme}.jpg`, type: 'jpeg', quality: 88, clip: around(81, 54) });
        await snap(page, { path: `ctx/${name.toLowerCase()}-${theme}.png`, clip: around(243, 162) });
        console.log(theme, name);
    }
}
console.log(log.slice(0, 5).join('\n'));
await browser.close();
