import { expect, test } from '@playwright/test';
import { worldPixel } from '../../../src/lib/planner/layers/mercator.ts';
import { instantAt, solarPosition, solarTerms } from '../../../src/lib/planner/solar.ts';
import { errorsOf, FLAT, routeFixture } from './harness.js';

// Down onto the middle of the fixture from high up, so that ground pages hold all of it, on a day of a low sun.
const EYE = [8.415, 46.7975];
const DATE = '2026-11-20';
const OVER = `?lat=${EYE[1]}&lon=${EYE[0]}&heading=0.0&pitch=-89.0&alt=9000.0&date=${DATE}&time=09:30&weather=sunny`;
const TIMES = ['09:30', '10:30', '11:30', '12:30', '13:30', '14:30', '15:30'];
// PENUMBRA of look.wgsl: the half width of the soft edge, in radians of the sun's altitude. And half a
// horizon texel of the finest ground pages (52 m at 47° N): the horizons spread a shadow's edge over it.
const PENUMBRA = 0.017, SPREAD_M = 26, METRES_PER_DEGREE = 111_320;
// The fixture's terrain tiles at zoom 12: x 2143 to 2144, y 1443 to 1444.
const TILE = 512, ZOOM = 12, FIRST = [2143, 1443], TILES = 2, SIDE = TILES * TILE;
// Far below any terrain: where the fixture has no tile, nothing blocks the sun.
const LOW = -1e4;

/** The heights of the fixture at zoom 12, decoded in the page: SIDE × SIDE pixels from tile FIRST. */
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

/** The height at a pixel centre of zoom 12. */
const heightOf = heights => (x, y) => {
    const i = x - FIRST[0] * TILE, j = y - FIRST[1] * TILE;
    return i >= 0 && j >= 0 && i < SIDE && j < SIDE ? heights[j * SIDE + i] : LOW;
};

const CURVE = 1 / (2 * 6371008.8);

/**
 * The reference: whether terrain blocks a ray to the sun from 1 m over the bilinear surface. The ray
 * crosses each pixel cell and compares with the exact maximum of the cell's bilinear patch, with Earth curvature.
 */
function shaded(height, [lon, lat], sun, distance) {
    const [wx, wy] = worldPixel([lon, lat], TILE * 2 ** ZOOM), px = wx - .5, py = wy - .5;
    const corners = (x, y) => [height(x, y), height(x + 1, y), height(x, y + 1), height(x + 1, y + 1)];
    const x0 = Math.floor(px), y0 = Math.floor(py), fx = px - x0, fy = py - y0, h = corners(x0, y0);
    const observer = h[0] * (1 - fx) * (1 - fy) + h[1] * fx * (1 - fy) + h[2] * (1 - fx) * fy + h[3] * fx * fy + 1;
    const pitch = 2 * Math.PI * 6378137 * Math.cos(lat * Math.PI / 180) / (TILE * 2 ** ZOOM);
    const dx = Math.sin(sun.bearing) / pitch, dy = -Math.cos(sun.bearing) / pitch, slope = Math.tan(sun.altitude);
    for (let lo = 0; lo < distance;) {
        const x = Math.floor(px + dx * (lo + 1e-5)), y = Math.floor(py + dy * (lo + 1e-5));
        const tx = dx > 0 ? (x + 1 - px) / dx : dx < 0 ? (x - px) / dx : Infinity;
        const ty = dy > 0 ? (y + 1 - py) / dy : dy < 0 ? (y - py) / dy : Infinity;
        const hi = Math.min(tx, ty, distance), v = corners(x, y), [ox, oy] = [px - x, py - y];
        // The patch height minus the ray along the segment is a quadratic in the distance t.
        const e = v[1] - v[0], n = v[2] - v[0], cross = v[3] - v[2] - v[1] + v[0];
        const a = cross * dx * dy - CURVE, b = e * dx + n * dy + cross * (ox * dy + oy * dx) - slope;
        const above = t => (a * t + b) * t + v[0] + e * ox + n * oy + cross * ox * oy - observer;
        const peak = -b / (2 * a);
        if (Math.max(above(lo), above(hi), a < 0 && peak > lo && peak < hi ? above(peak) : -Infinity) > 0) return true;
        lo = hi;
    }
    return false;
}

test('the terrain shadows agree with a reference ray march outside the penumbra, in the first frame after each change of the time', { tag: '@gpu' }, async ({ page }) => {
    test.slow();
    await page.setViewportSize({ width: 480, height: 300 });
    const errors = errorsOf(page);
    await routeFixture(page, undefined, FLAT);
    await page.goto(`/trailview.html${OVER}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
    await page.evaluate(() => window.trailView.waitForIdle());
    expect((await page.evaluate(() => window.trailView.stats())).ground.resident).toBeGreaterThan(0);
    const height = heightOf(await fixtureHeights(page));
    // A grid over the fixture, 400 m inside its bounds.
    const points = [];
    for (let j = 0; j < 16; j++) for (let i = 0; i < 16; i++) points.push([8.376 + 0.078 * i / 15, 46.769 + 0.057 * j / 15]);
    // Outside the penumbra of the sun's altitude alone (angle), and of that or a shadow edge (edge): the
    // points, those in shade, those that the shader lights in shade (lit) and shades in the sun (shaded).
    const counts = { edge: { checked: 0, shade: 0, lit: 0, shaded: 0 }, angle: { checked: 0, shade: 0, lit: 0, shaded: 0 } };
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
        expect(sun.altitude - PENUMBRA).toBeGreaterThan(0);
        const state = ([lon, lat], altitude) => shaded(height, [lon, lat], { ...sun, altitude }, 10_000);
        points.forEach(([lon, lat], i) => {
            // The penumbra: the sun within PENUMBRA of the horizon, and for edge also a shadow edge within SPREAD_M.
            const step = [SPREAD_M / (METRES_PER_DEGREE * Math.cos(lat * Math.PI / 180)), SPREAD_M / METRES_PER_DEGREE];
            const states = [state([lon, lat], sun.altitude - PENUMBRA), state([lon, lat], sun.altitude + PENUMBRA),
                ...[[1, 0], [-1, 0], [0, 1], [0, -1]].map(([x, y]) => state([lon + x * step[0], lat + y * step[1]], sun.altitude))];
            const tally = count => {
                count.checked++;
                if (states[0]) count.shade++;
                if (shares[i] > 0.5 && states[0]) count.lit++;
                if (shares[i] <= 0.5 && !states[0]) count.shaded++;
            };
            if (states[0] === states[1]) tally(counts.angle);
            if (states.every(value => value === states[0])) tally(counts.edge);
        });
    }
    console.log(JSON.stringify(counts));
    const { edge } = counts;
    expect(edge.shade, JSON.stringify(counts)).toBeGreaterThan(0.05 * edge.checked);
    expect(edge.lit + edge.shaded, JSON.stringify(counts)).toBeLessThanOrEqual(0.02 * edge.checked);
    expect(errors).toEqual([]);
});
