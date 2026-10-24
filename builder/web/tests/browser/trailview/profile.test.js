// Diagnostic only, never merged: prints where the smoke view's time goes on the runner's SwiftShader.
import { test } from '@playwright/test';
import { routeFixture } from './harness.js';

const DOWN = '?lat=46.797500&lon=8.415000&heading=20.0&pitch=-60.0&alt=2500.0&date=2026-08-15&time=15:00&weather=sunny';

test('profile of the smoke view', async ({ page }) => {
    test.setTimeout(600_000);
    const t0 = Date.now(), log = m => console.log(`PROFILE ${((Date.now() - t0) / 1000).toFixed(1)}s ${m}`);
    page.on('console', m => m.text().startsWith('PIPE') && log(m.text()));
    await page.addInitScript(() => {
        for (const name of ['createRenderPipelineAsync', 'createComputePipelineAsync']) {
            const f = GPUDevice.prototype[name];
            GPUDevice.prototype[name] = async function (d) { const a = performance.now(); try { return await f.call(this, d); } finally { console.log(`PIPE start ${(a / 1000).toFixed(1)} took ${((performance.now() - a) / 1000).toFixed(2)} ${d.label ?? '?'}`); } };
        }
    });
    await routeFixture(page);
    await page.goto(`/trailview.html${DOWN}`);
    await page.waitForFunction(() => window.trailView, null, { timeout: 300_000, polling: 200 });
    log('engine ready');
    const frames = await page.evaluate(async () => {
        const tv = window.trailView, out = [];
        let last = performance.now();
        for (let k = 0; k < 400; k++) {
            await tv.nextFrame();
            const now = performance.now(), dt = now - last;
            last = now;
            out.push(Math.round(dt) + (dt > 2000 ? JSON.stringify(Object.fromEntries(Object.entries(tv.stats().gpu).map(([p, v]) => [p, Math.round(v)]))) : ''));
            if (tv.terrain.idle && tv.groundLook.idle && !tv.converging && tv.trees.idle && tv.instances.idle && tv.houses.idle) break;
        }
        return out;
    });
    log(`frames ${frames.length}: ${frames.join(' ')}`);
    await page.evaluate(() => window.trailView.waitForIdle());
    log('idle');
});
