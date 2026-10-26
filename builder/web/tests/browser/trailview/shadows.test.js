import { expect, test } from '@playwright/test';
import { instantAt, SHADE, SUN, solarPosition, solarTerms, TILE, visibility } from '../../../src/lib/planner/layers/sun.ts';
import { errorsOf, FLAT, routeFixture } from './harness.js';

// Down onto the middle of the fixture from high up, so that ground pages hold all of it, on a day of a low sun.
const EYE = [8.415, 46.7975];
const DATE = '2026-11-20';
const OVER = `?lat=${EYE[1]}&lon=${EYE[0]}&heading=0.0&pitch=-89.0&alt=9000.0&date=${DATE}&time=09:30&weather=sunny`;
const TIMES = ['09:30', '10:30', '11:30', '12:30', '13:30', '14:30', '15:30'];
// PENUMBRA of look.wgsl: the half width of the soft edge, in radians of the sun's altitude. And half a
// horizon texel of the finest ground pages (52 m at 47° N): the horizons spread a shadow's edge over it.
const PENUMBRA = 0.017, SPREAD_M = 26, METRES_PER_DEGREE = 111_320;
// The fixture's tiles of the native zoom of visibility(): x 2143 to 2144, y 1443 to 1444.
const FIRST = [2143, 1443], TILES = 2, SIDE = TILES * TILE;
// Far below any terrain: where the fixture has no tile, nothing blocks the sun.
const LOW = -1e4;

/** The heights of the fixture at the native zoom of visibility(), decoded in the page: SIDE × SIDE pixels from tile FIRST. */
async function fixtureHeights(page) {
    const values = await page.evaluate(async ({ first, tiles, tile, low }) => {
        const out = new Float32Array(tiles * tiles * tile * tile);
        for (let j = 0; j < tiles; j++) for (let i = 0; i < tiles; i++) {
            const body = await (await fetch(`https://tiles.test/terrain/12/${first[0] + i}/${first[1] + j}.webp`)).blob();
            const image = await createImageBitmap(body, { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
            const context = new OffscreenCanvas(tile, tile).getContext('2d');
            context.drawImage(image, 0, 0);
            const rgba = context.getImageData(0, 0, tile, tile).data;
            for (let p = 0; p < tile * tile; p++) {
                const h = rgba[4 * p + 3] === 255 ? rgba[4 * p] * 256 + rgba[4 * p + 1] + rgba[4 * p + 2] / 256 - 32768 : low;
                out[((j * tile) + Math.floor(p / tile)) * tiles * tile + i * tile + (p % tile)] = h;
            }
        }
        return Array.from(out);
    }, { first: FIRST, tiles: TILES, tile: TILE, low: LOW });
    return Float32Array.from(values);
}

/** The Surface of visibility() at its native pixels; the march over each pixel needs no block maxima. */
const surfaceOf = heights => ({
    height(level, x, y) {
        const i = x - FIRST[0] * TILE, j = y - FIRST[1] * TILE;
        return i >= 0 && j >= 0 && i < SIDE && j < SIDE ? heights[j * SIDE + i] : LOW;
    },
});

test('the terrain shadows agree with visibility() of the planner outside the penumbra, in the first frame after each change of the time', { tag: '@gpu' }, async ({ page }) => {
    test.slow();
    await page.setViewportSize({ width: 480, height: 300 });
    const errors = errorsOf(page);
    await routeFixture(page, undefined, FLAT);
    await page.goto(`/trailview.html${OVER}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
    await page.evaluate(() => window.trailView.waitForIdle());
    expect((await page.evaluate(() => window.trailView.stats())).ground.resident).toBeGreaterThan(0);
    const surface = surfaceOf(await fixtureHeights(page));
    // A grid over the fixture, 400 m inside its bounds.
    const points = [];
    for (let j = 0; j < 16; j++) for (let i = 0; i < 16; i++) points.push([8.376 + 0.078 * i / 15, 46.769 + 0.057 * j / 15]);
    const counts = { checked: 0, agree: 0, shade: 0, penumbra: 0 };
    for (const time of TIMES) {
        await page.evaluate(async time => {
            const tv = window.trailView;
            tv.view = { ...tv.view, time };
            await tv.nextFrame();
        }, time);
        const shares = [];
        for (let k = 0; k < points.length; k += 64) shares.push(...(await page.evaluate(chunk => window.trailView.check(chunk), points.slice(k, k + 64))).sun);
        const [hours, minutes] = time.split(':').map(Number);
        const sun = solarPosition(EYE, solarTerms(instantAt(DATE, 60 * hours + minutes, 'Europe/Zurich')));
        const state = ([lon, lat], altitude) => visibility(surface, [lon, lat], { ...sun, altitude }, 10_000, undefined, false);
        points.forEach(([lon, lat], i) => {
            // In the penumbra: the sun within PENUMBRA of the horizon, or a shadow edge within SPREAD_M.
            const step = [SPREAD_M / (METRES_PER_DEGREE * Math.cos(lat * Math.PI / 180)), SPREAD_M / METRES_PER_DEGREE];
            const states = [state([lon, lat], sun.altitude - PENUMBRA), state([lon, lat], sun.altitude + PENUMBRA),
                ...[[1, 0], [-1, 0], [0, 1], [0, -1]].map(([x, y]) => state([lon + x * step[0], lat + y * step[1]], sun.altitude))];
            if (states.some(value => value !== states[0])) return void counts.penumbra++;
            expect([SUN, SHADE]).toContain(states[0]);
            counts.checked++;
            if (states[0] === SHADE) counts.shade++;
            if ((shares[i] > 0.5) === (states[0] === SUN)) counts.agree++;
        });
    }
    expect(counts.shade, JSON.stringify(counts)).toBeGreaterThan(0.05 * counts.checked);
    expect(counts.agree, JSON.stringify(counts)).toBeGreaterThanOrEqual(0.98 * counts.checked);
    expect(errors).toEqual([]);
});
