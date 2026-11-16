// node measure.mjs counts | moves | smooth
import { AREAS, launch, open, settle, view } from './lib.mjs';
const what = process.argv[2];
const browser = await launch();
if (what === 'counts') {
    for (const [area, center] of Object.entries(AREAS)) {
        const { page, context } = await open(browser, { mode: 'groups', center, zoom: 13 });
        console.log(area, JSON.stringify(await page.evaluate(() => ({ tiles: window.__places.loadedTiles.length, ...window.__places.stats(), firstLoad: window.__places.timings[0] }))));
        await context.close();
    }
}
if (what === 'moves') {
    // A move to a new area (new place tiles), then a move inside the loaded area.
    const timed = (page, center, zoom) => page.evaluate(([center, zoom]) => new Promise(resolve => {
        const { map } = window.__places, start = performance.now(), before = window.__places.timings.length;
        map.jumpTo({ center, zoom });
        const done = () => (map.loaded() && !map.isMoving() && !window.__places.busy ? resolve({ idleMs: Math.round(performance.now() - start), places: window.__places.timings.slice(before) }) : setTimeout(done, 20));
        setTimeout(done, 20);
    }), [center, zoom]);
    for (const mode of ['today', 'groups', 'badges']) for (const kinds of ['default', 'all']) {
        const { page, context } = await open(browser, { mode, kinds, center: AREAS.mountains, zoom: 13 });
        const far = await timed(page, AREAS.town, 14);
        await settle(page);
        const near = await timed(page, [AREAS.town[0] + 0.004, AREAS.town[1] - 0.002], 15);
        const back = await timed(page, AREAS.town, 14);
        console.log(mode, kinds, 'new area:', JSON.stringify(far), '| loaded area z15:', JSON.stringify(near), '| loaded area z14:', JSON.stringify(back));
        await context.close();
    }
}
if (what === 'smooth') {
    // Frame times while the map zooms from 13 to 16 and pans, in the town with all 13 kinds.
    for (const kinds of ['all', 'default']) for (const mode of ['today', 'groups', 'badges']) {
        const { page, context } = await open(browser, { mode, kinds, center: AREAS.town, zoom: 13, scale: 1 });
        for (const run of [0, 1]) {
            const frames = await page.evaluate(([center]) => new Promise(resolve => {
                const { map } = window.__places, deltas = [];
                let last = performance.now(), moving = true;
                const tick = now => { deltas.push(now - last); last = now; if (moving) requestAnimationFrame(tick); else resolve(deltas); };
                requestAnimationFrame(tick);
                map.easeTo({ center: [center[0] + 0.006, center[1] - 0.004], zoom: 16, duration: 3000, easing: t => t });
                map.once('moveend', () => { map.easeTo({ center, zoom: 13, duration: 3000, easing: t => t }); map.once('moveend', () => { moving = false; }); });
            }), [AREAS.town]);
            const sorted = [...frames].sort((a, b) => a - b), at = q => Math.round(sorted[Math.floor(sorted.length * q)]);
            if (run) console.log(mode, kinds, JSON.stringify({ frames: frames.length, fps: Math.round(frames.length / 6), medianMs: at(0.5), p95Ms: at(0.95), worstMs: Math.round(sorted.at(-1)), over50ms: frames.filter(d => d > 50).length }));
            await settle(page);
        }
        await context.close();
    }
}
await browser.close();
