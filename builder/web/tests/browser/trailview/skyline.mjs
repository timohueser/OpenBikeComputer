// The skyline check: the skyline that TrailView draws at each `peak-view-photos` position against the
// skyline read off its photograph (fixtures/sources/peak-view/photos/), as the RMS of the elevation
// difference over the photograph's columns, against its `rms_limit_deg`.
//
//   node builder/web/tests/browser/trailview/skyline.mjs [--tiles FILE] [--lab LAB] [--gpu]
//
// The eye stands 1.6 m above the photograph's `ground_m`, at its heading and pitch. The page draws
// with the red clear colour, so the drawn surface is every pixel that is not red. Each pixel gives a
// bearing and an elevation through the camera; the skyline of a quarter-degree bearing is its highest
// terrain pixel. Prints one JSON line per photograph and exits 1 when one is past its limit.

import { readdirSync, readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright';
import { EXECUTABLE, routeFixture, serve, SWIFTSHADER } from './harness.js';

const PHOTOS = new URL('../../../../../fixtures/sources/peak-view/photos/', import.meta.url).pathname;
const PHONE_M = 1.6;
const { values } = parseArgs({ options: { tiles: { type: 'string' }, lab: { type: 'string' }, gpu: { type: 'boolean' } } });
const photos = readdirSync(PHOTOS).filter(file => file.endsWith('.json')).map(file => ({ name: file.slice(0, -5), ...JSON.parse(readFileSync(PHOTOS + file, 'utf8')) }));
const { origin, close } = await serve();
const browser = await chromium.launch({ executablePath: EXECUTABLE, headless: !values.gpu, args: values.gpu ? [] : SWIFTSHADER });
let failed = 0;
try {
    for (const photo of photos) {
        const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });
        await routeFixture(page, values.tiles);
        const view = `?lat=${photo.lat}&lon=${photo.lon}&heading=${photo.heading_deg}&pitch=${photo.pitch_deg}&alt=${photo.ground_m + PHONE_M}`
            + `&date=2026-08-15&time=15:00&weather=sunny&debug=cracks${values.lab ? `&lab=${values.lab}` : ''}`;
        await page.goto(`${origin}/trailview.html${view}`);
        await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
        await page.evaluate(() => window.trailView.waitForIdle());
        await page.addStyleTag({ content: '#hud, #credits { display: none; }' });
        const png = (await page.screenshot()).toString('base64');
        const skyline = await page.evaluate(async ({ png, vfov }) => {
            const image = await createImageBitmap(await (await fetch(`data:image/png;base64,${png}`)).blob());
            const context = new OffscreenCanvas(image.width, image.height).getContext('2d');
            context.drawImage(image, 0, 0);
            const { data, width, height } = context.getImageData(0, 0, image.width, image.height);
            const { heading, pitch } = window.trailView.view, rad = Math.PI / 180, h = heading * rad, p = pitch * rad;
            const forward = [Math.sin(h) * Math.cos(p), Math.cos(h) * Math.cos(p), Math.sin(p)], right = [Math.cos(h), -Math.sin(h), 0];
            const up = [right[1] * forward[2] - right[2] * forward[1], right[2] * forward[0] - right[0] * forward[2], right[0] * forward[1] - right[1] * forward[0]];
            const ty = Math.tan(vfov * rad / 2), tx = ty * width / height, bins = {};
            for (let x = 0; x < width; x++) {
                let y = 0;
                while (y < height && data[4 * (y * width + x)] === 255 && data[4 * (y * width + x) + 1] === 0) y++;
                // Terrain at the top of the frame: the skyline is above the view.
                if (y === 0 || y === height) continue;
                const nx = (x + 0.5) / width * 2 - 1, ny = 1 - y / height * 2;
                const d = [0, 1, 2].map(i => forward[i] + nx * tx * right[i] + ny * ty * up[i]);
                const bearing = (Math.atan2(d[0], d[1]) / rad + 360) % 360, elevation = Math.asin(d[2] / Math.hypot(...d)) / rad;
                const bin = Math.round(bearing * 4);
                bins[bin] = Math.max(bins[bin] ?? -90, elevation);
            }
            return bins;
        }, { png, vfov: 60 });
        const differences = photo.columns.flatMap(([bearing, elevation]) => {
            const drawn = skyline[Math.round(bearing * 4)];
            return drawn === undefined ? [] : [drawn - elevation];
        });
        const rms = Math.sqrt(differences.reduce((sum, d) => sum + d * d, 0) / differences.length);
        const pass = differences.length === photo.columns.length && rms <= photo.rms_limit_deg;
        if (!pass) failed++;
        console.log(JSON.stringify({ photo: photo.name, rms: +rms.toFixed(3), limit: photo.rms_limit_deg, columns: photo.columns.length, drawn: differences.length, pass }));
        await page.close();
    }
} finally {
    await browser.close();
    close();
}
process.exit(failed ? 1 : 0);
