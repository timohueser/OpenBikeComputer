// node badges.mjs maps [SHAPE,..] | closeup | list | motion: the pictures of the badge shapes.
import { execFileSync } from 'node:child_process';
import { renameSync, writeFileSync } from 'node:fs';
import { AREAS, OUT, launch, open, view, shot, settle } from './lib.mjs';
const [what, only] = process.argv.slice(2);
const SHAPES = (only ?? 'pill,tile,stack,ring').split(',');
const browser = await launch();
const errors = log => { const bad = log.filter(line => !line.startsWith('places prototype')); if (bad.length) console.log(bad.slice(0, 5).join('\n')); };

/** How many marks the view has, and how many of them touch another mark. */
const marks = page => page.evaluate(() => {
    const items = window.__places.badges.info(), box = item => [item.x - item.w / 2, item.y - item.h / 2, item.x + item.w / 2, item.y + item.h / 2];
    const boxes = items.map(box);
    const touching = boxes.filter((a, i) => boxes.some((b, j) => i !== j && Math.min(a[2], b[2]) > Math.max(a[0], b[0]) && Math.min(a[3], b[3]) > Math.max(a[1], b[1]))).length;
    return { marks: items.length, places: items.reduce((sum, item) => sum + item.count, 0), badges: items.filter(item => item.count > 1).length, touching, widest: Math.max(...items.map(item => item.w)), mostKinds: Math.max(...items.map(item => item.kinds.split('.').length)) };
});

if (what === 'maps') {
    const report = {};
    for (const [scheme, views] of [['light', [['town', 13], ['town', 14], ['town', 15], ['town', 16], ['valley', 12], ['valley', 13], ['valley', 14]]], ['dark', [['town', 14]]]]) {
        const { page, context, log } = await open(browser, { mode: 'badges', scheme });
        for (const [area, zoom] of views) {
            await view(page, AREAS[area], zoom);
            for (const shape of SHAPES) {
                await page.evaluate(shape => window.__places.badges.setShape(shape), shape);
                await settle(page);
                const name = `${area}-z${zoom}-${shape}${scheme === 'dark' ? '-dark' : ''}`;
                await shot(page, name);
                report[name] = await marks(page);
                console.log(name, JSON.stringify(report[name]));
            }
        }
        errors(log);
        await context.close();
    }
    writeFileSync(`${OUT}/report-${SHAPES.join(',')}.json`, JSON.stringify(report, null, 1));
}

if (what === 'closeup') {
    // One row of specimens for each shape and theme on a quiet piece of the real map, three times the scale of the map pictures.
    const ALL = ['water', 'shop', 'food', 'camp', 'bike', 'station', 'peak'];
    const row = shape => [
        { kinds: ALL.slice(0, 1), count: 1, x: 44 }, { kinds: ALL.slice(0, 2), count: 2, x: 116 }, { kinds: ALL.slice(0, 3), count: 3, x: 198 },
        { kinds: ALL.slice(0, 4), count: 4, x: 290 }, { kinds: ALL, count: 7, x: 400 }, { kinds: ['food'], count: 3, x: 506 },
        ...(shape === 'tile' ? [{ kinds: ALL, count: 7, x: 584, tileMax: 4 }] : []),
    ];
    for (const scheme of ['light', 'dark']) {
        const { page, context, log } = await open(browser, { mode: 'badges', scheme, scale: 6, viewport: { width: 1000, height: 640 } });
        await view(page, [8.2835, 47.0405], 14.6);
        // No map label lies under a specimen.
        await page.evaluate(() => { const { map } = window.__places; for (const layer of map.getStyle().layers) if (layer.type === 'symbol') map.setLayoutProperty(layer.id, 'visibility', 'none'); });
        const box = await page.locator('.map-area').boundingBox();
        for (const shape of SHAPES) {
            await page.evaluate(([shape, items]) => { window.__places.badges.setShape(shape); window.__places.badges.specimen(items, 300); }, [shape, row(shape)]);
            await settle(page);
            await page.screenshot({ path: `${OUT}/work/closeup-${shape}-${scheme}.png`, clip: { x: box.x + 4, y: box.y + 262, width: 624, height: 76 } });
            console.log('closeup', shape, scheme);
        }
        errors(log);
        await context.close();
    }
}

/** The badge of the middle of the view with the most places, up to `most`. */
const badge = (page, most, least = 2) => page.evaluate(([most, least]) => window.__places.badges.info()
    .filter(item => item.count >= least && item.count <= most && item.x > 0.3 * window.__places.map.getContainer().clientWidth && item.x < 0.7 * window.__places.map.getContainer().clientWidth && item.y > 120 && item.y < 300)
    .sort((a, b) => b.count - a.count)[0], [most, least]);
const canvas = page => page.locator('.map-area canvas.maplibregl-canvas').boundingBox();

if (what === 'list') {
    for (const scheme of ['light', 'dark']) {
        const { page, context, log } = await open(browser, { mode: 'badges', extra: `&badge=${only ?? 'tile'}`, scheme });
        await view(page, AREAS.town, 15);
        const box = await canvas(page), item = await badge(page, 40, 12);
        console.log('list on', JSON.stringify(item));
        await page.mouse.click(box.x + item.x, box.y + item.y);
        await page.waitForTimeout(600);
        await shot(page, `list-open-${scheme}`);
        if (scheme === 'light') {
            await page.locator('.places-list button').nth(3).hover();
            await page.waitForTimeout(200);
            await shot(page, 'list-hover-light');
            await page.locator('.places-list button').nth(3).click();
            await page.waitForTimeout(900);
            await shot(page, 'list-picked-light');
            await page.keyboard.press('Escape');
        }
        errors(log);
        await context.close();
    }
}

if (what === 'motion') {
    // Each motion at its real speed, then five times slower. The video is the part after the page has loaded, cut to the place of the motion.
    for (const name of (only ?? 'list-open,separation').split(',')) {
        // A small map with no relief: this machine draws the map in software, and the motion needs the frames.
        const viewport = { width: 900, height: 660 };
        const { page, context, log } = await open(browser, { mode: 'badges', extra: '&badge=tile', scale: 1, viewport, video: OUT });
        await page.evaluate(() => { const { map } = window.__places; map.setTerrain(null); for (const layer of map.getStyle().layers) if (layer.type === 'hillshade' || /contour/.test(layer.id)) map.setLayoutProperty(layer.id, 'visibility', 'none'); });
        await page.evaluate(() => { const note = document.createElement('div'); note.id = 'speed'; note.style.cssText = 'position:absolute;left:10px;top:10px;z-index:9;padding:4px 10px;border-radius:6px;background:#22211c;color:#fff;font:600 13px sans-serif'; document.querySelector('.map-area').append(note); });
        const speed = (slow, text) => page.evaluate(([slow, text]) => { window.__places.badges.slow = slow; document.getElementById('speed').textContent = text; }, [slow, text]);
        await view(page, AREAS.town, name === 'list-open' ? 15 : 14.7);
        const box = await canvas(page);
        await page.mouse.move(box.x + 20, box.y + box.height - 20);
        await speed(1, 'real speed');
        await page.waitForTimeout(800);
        const began = Date.now();
        for (const [slow, text] of [[1, 'real speed'], [5, '5 times slower']]) {
            await speed(slow, text);
            await page.waitForTimeout(600);
            if (name === 'list-open') {
                const item = await badge(page, 40, 12);
                await page.mouse.move(box.x + item.x, box.y + item.y, { steps: 8 });
                await page.mouse.down(); await page.mouse.up();
                await page.waitForTimeout(1400);
                await page.keyboard.press('Escape');
                await page.waitForTimeout(1000);
                await page.mouse.down(); await page.mouse.up();
                await page.waitForTimeout(1400);
                await page.locator('.places-list button').nth(4).click();
                await page.waitForTimeout(1600);
                await page.locator('.maplibregl-popup button.close').click().catch(() => {});
                await page.mouse.move(box.x + 20, box.y + box.height - 20);
                await view(page, AREAS.town, 15);
            } else {
                for (const zoom of [15.3, 16.2, 15.3, 14.7]) {
                    const frames = await page.evaluate(zoom => new Promise(resolve => {
                        const { map } = window.__places;
                        let frames = 0, moving = true;
                        const tick = () => { frames++; if (moving) requestAnimationFrame(tick); };
                        requestAnimationFrame(tick);
                        map.once('moveend', () => { moving = false; resolve(frames); });
                        map.easeTo({ zoom, duration: 450 });
                    }), zoom);
                    console.log('zoom to', zoom, 'frames in 450 ms:', frames);
                    await page.waitForTimeout(1500);
                }
            }
        }
        const length = (Date.now() - began) / 1000 + 1;
        errors(log);
        const video = await page.video().path();
        await context.close();
        const total = Number(execFileSync('ffprobe', ['-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', video]).toString());
        const size = execFileSync('ffprobe', ['-v', 'error', '-select_streams', 'v', '-show_entries', 'stream=width,height', '-of', 'csv=p=0', video]).toString().trim().split(',').map(Number);
        const crop = [box.x, box.y, box.width, box.height].map(value => Math.floor(value / 2) * 2);
        execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-ss', Math.max(0, total - length).toFixed(2), '-i', video, '-vf', `crop=${crop[2]}:${crop[3]}:${crop[0]}:${crop[1]},scale=iw*2:ih*2:flags=lanczos`, '-c:v', 'libvpx-vp9', '-b:v', '0', '-crf', '28', `${OUT}/${name}.webm`]);
        renameSync(video, `${OUT}/raw-${name}.webm`);
        console.log(name, `${length.toFixed(1)} s of ${total.toFixed(1)} s`, size.join('x'));
    }
}

if (what === 'cost') {
    // What the HTML marks cost: the frames of a zoom from 13 to 16 and back with a pan, in the town, against the map of today.
    for (const [mode, extra] of [['today', ''], ['badges', '&badge=tile']]) {
        const { page, context } = await open(browser, { mode, extra, center: AREAS.town, zoom: 13, scale: 1 });
        for (const run of [0, 1]) {
            const frames = await page.evaluate(([center]) => new Promise(resolve => {
                const { map, badges } = window.__places, deltas = [];
                window.__spent = 0;
                if (badges && !badges.timed) for (const name of ['position', 'update']) { const run = badges[name].bind(badges); badges.timed = true; badges[name] = (...args) => { const start = performance.now(); const depth = window.__depth = (window.__depth ?? 0) + 1; const result = run(...args); window.__depth--; if (depth === 1) window.__spent += performance.now() - start; return result; }; }
                let last = performance.now(), moving = true, most = 0;
                const tick = now => { deltas.push(now - last); last = now; most = Math.max(most, document.querySelectorAll('.places-mark').length); if (moving) requestAnimationFrame(tick); else resolve({ deltas, most, spent: window.__spent }); };
                requestAnimationFrame(tick);
                map.easeTo({ center: [center[0] + 0.006, center[1] - 0.004], zoom: 16, duration: 3000, easing: t => t });
                map.once('moveend', () => { map.easeTo({ center, zoom: 13, duration: 3000, easing: t => t }); map.once('moveend', () => { moving = false; }); });
            }), [AREAS.town]);
            const sorted = [...frames.deltas].sort((a, b) => a - b), at = q => Math.round(sorted[Math.floor(sorted.length * q)]);
            if (run) console.log(mode, JSON.stringify({ frames: sorted.length, fps: Math.round(sorted.length / 6), medianMs: at(0.5), p95Ms: at(0.95), worstMs: Math.round(sorted.at(-1)), over50ms: sorted.filter(d => d > 50).length, mostMarkElements: frames.most, markScriptMsPerFrame: Math.round(frames.spent / sorted.length * 10) / 10 }));
            await settle(page);
        }
        await context.close();
    }
}
await browser.close();
