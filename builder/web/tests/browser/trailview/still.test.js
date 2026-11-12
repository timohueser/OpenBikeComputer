import { expect, test } from '@playwright/test';
import { errorsOf, routeFixture } from './harness.js';

// From over Engelberg, south to the slope under the Trübsee plateau: the line climbs the slope toward
// the eye, then runs on behind the edge of the plateau, at about 46.8035° N.
const RIDGE = { lat: 46.828, lon: 8.385, heading: 180, pitch: -6, mode: 'fly', height: 1700, date: '2026-08-15', time: '10:00', weather: 'sunny', debug: 'off' };
const BEFORE = [[8.385, 46.814], [8.385, 46.81], [8.385, 46.806]];
const BEHIND = [[8.385, 46.799], [8.385, 46.796], [8.385, 46.793]];
// Along the valley floor of Engelberg, west to east.
const ROUTE = [[8.375, 46.821], [8.385, 46.82], [8.395, 46.819], [8.405, 46.818], [8.415, 46.816], [8.425, 46.813], [8.432, 46.81]];
// SwiftShader takes minutes for a still.
const SLOW = { timeoutMs: 600_000 };

async function open(page) {
    await routeFixture(page);
    await page.goto('/trailview.html?still');
    await page.waitForFunction(() => window.trailviewStill, null, { timeout: 60_000 });
}

/** A still in the page: the type and the size of its image, and its points as arrays. */
function render(page, job) {
    return page.evaluate(async ([job, options]) => {
        const { image, points } = await window.trailviewStill.still(job, options);
        const { width, height } = await createImageBitmap(image);
        return { type: image.type, width, height, points: points && Object.fromEntries(Object.entries(points).map(([key, values]) => [key, Array.from(values)])) };
    }, [job, SLOW]);
}

const inside = ({ x, y }, i, width, height) => x[i] >= 0 && x[i] < width && y[i] >= 0 && y[i] < height;

test('a still has the size of its job, and the points behind a ridge are hidden', { tag: '@gpu' }, async ({ page }) => {
    test.setTimeout(900_000);
    const errors = errorsOf(page);
    await open(page);
    const still = await render(page, { view: RIDGE, width: 480, height: 270, line: [...BEFORE, ...BEHIND] });
    expect([still.width, still.height]).toEqual([480, 270]);
    expect(['image/webp', 'image/jpeg']).toContain(still.type);
    const { points } = still;
    for (const i of BEFORE.keys()) {
        expect(points.visible[i], `point ${i}`).toBe(1);
        expect(inside(points, i, 480, 270)).toBe(true);
    }
    // The line runs away from the eye.
    expect(points.distance.every((d, i) => !i || d > points.distance[i - 1])).toBe(true);
    for (const i of BEHIND.keys()) expect(points.visible[BEFORE.length + i], `point ${BEFORE.length + i}`).toBe(0);
    expect(errors).toEqual([]);
});

test('the engine draws the route of a cover view inside the image, as the view search placed it', { tag: '@gpu' }, async ({ page }) => {
    test.setTimeout(900_000);
    const errors = errorsOf(page);
    await open(page);
    const size = { width: 480, height: 320 };
    const view = await page.evaluate(([line, when]) => window.trailviewStill.coverView(line, when), [ROUTE, { ...size, date: '2026-08-15', time: '10:00', weather: 'sunny' }]);
    expect(view).toBeDefined();
    // Points every few metres, as the line of a summary.
    const line = ROUTE.slice(1).flatMap((to, k) => Array.from({ length: 20 }, (_, s) => ROUTE[k].map((from, j) => from + (to[j] - from) * s / 20)));
    const { points } = await render(page, { view, ...size, line });
    const shown = line.filter((_, i) => inside(points, i, size.width, size.height)).length;
    expect(shown / line.length).toBeGreaterThanOrEqual(0.85);
    expect(errors).toEqual([]);
});
