// The GPU half of the TrailView fault census: reads the samples that scripts/road-census.ts wrote to
// SAMPLES_DIR, probes the drawn terrain and the mask at them, and prints the tears, the lifts and the plants in
// water over the fixture, one JSON line each, as {"count":"tears","samples":767,"places":86}.
//
//   npm run pretest:trailview                    # build against the fixture
//   flock /tmp/trailview-gpu.lock node trailview/census.mjs [--gpu] [--ground DIR] SAMPLES_DIR
//
// Per square of CELL_M metres, an eye EYE_M metres over the square's highest sample, at its middle, draws the
// terrain all round within REACH_M metres, at the finest level near it. Once the terrain is idle, the probe
// reads drawnHeight at the square's face samples and kept_off at its water samples. A sample farther from the
// eye than REACH_M, or a water sample past the mask pool's reach, stays unknown, as does a sample that no page
// knows. --gpu draws on the machine's GPU, else SwiftShader: the census then takes hours. Prints to stderr the
// time, the squares per terrain quality (a quality over 0 coarsens the levels) and the unknown samples.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { errorsOf, launch, routeFixture, serve } from './harness.js';

const { values, positionals } = parseArgs({ allowPositionals: true, options: { ground: { type: 'string' }, gpu: { type: 'boolean' } } });
if (positionals.length !== 1) throw new Error('usage: node trailview/census.mjs [--gpu] [--ground DIR] SAMPLES_DIR');

/** A tear: a face sample more than this many metres under the drawn terrain, which shows through the face. */
const TEAR_M = 0.1;
/** A lift: a face sample more than this many metres over the drawn terrain, which leaves the face in the air. */
const LIFT_M = 0.6;
/** As DECK_BIT of census.ts: a face sample near a deck, whose faces stand over the terrain by design. */
const DECK_BIT = 2 ** 20;
/** As PLACE_M of census.ts: a place is a square of this many metres with 2 or more samples of a count. */
const PLACE_M = 10;
/** A square's corner lies 212 m from its middle, well inside REACH_M. */
const CELL_M = 300, REACH_M = 340, EYE_M = 20, UNITS = 2 ** 30;

/** As metresPerUnit of bins.ts. */
const metresPerUnit = y => 2 * Math.PI * 6_371_000 / UNITS / Math.cosh(Math.PI * (1 - 2 * y / UNITS));
const lonOf = x => x / UNITS * 360 - 180, latOf = y => Math.atan(Math.sinh(Math.PI * (1 - 2 * y / UNITS))) * 180 / Math.PI;
const rowsOf = name => {
    const bytes = readFileSync(join(positionals[0], `${name}.bin`));
    return new Float64Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 8);
};
const sets = [{ rows: rowsOf('faces'), width: 4 }, { rows: rowsOf('water'), width: 3 }];
const results = sets.map(({ rows, width }) => new Float32Array(rows.length / width).fill(NaN));

// The samples per set, by their square of CELL_M metres, in world units at the first sample's latitude.
const squares = new Map(), cell = CELL_M / metresPerUnit(sets[0].rows[1]);
sets.forEach(({ rows, width }, s) => {
    for (let i = 0; i < rows.length / width; i++) {
        const name = `${Math.floor(rows[width * i] / cell)},${Math.floor(rows[width * i + 1] / cell)}`;
        (squares.get(name) ?? squares.set(name, [[], []]).get(name))[s].push(i);
    }
});

const start = Date.now(), { origin, close } = await serve(), browser = await launch(values.gpu), qualities = new Map();
try {
    const page = await browser.newPage({ viewport: { width: 640, height: 400 }, deviceScaleFactor: 1 });
    const errors = errorsOf(page);
    await routeFixture(page, undefined, values.ground);
    await page.goto(`${origin}/trailview.html?date=2026-08-15&time=10:30&weather=sunny`);
    await page.waitForFunction(() => window.trailView, null, { timeout: 60_000 });
    await page.evaluate(reach => {
        // All round within `reach` of the eye, and only the cover past it.
        const terrain = window.trailView.terrain;
        terrain.visible = (node, eye) => {
            const side = 2 ** (30 - node.level), { whole, part, unit } = eye.anchor, [x, y] = [whole[0] + part[0], whole[1] + part[1]];
            const d = (lo, e) => Math.max(lo - e, 0, e - lo - side);
            return Math.hypot(d(node.x * side, x), d(node.y * side, y)) * unit <= reach;
        };
    }, REACH_M);
    for (const [name, members] of squares) {
        const [i, j] = name.split(',').map(Number), [ex, ey] = [(i + 0.5) * cell, (j + 0.5) * cell], unit = metresPerUnit(ey);
        let top = -Infinity;
        const points = members.map((list, s) => {
            const { rows, width } = sets[s], out = new Float64Array(2 * list.length);
            list.forEach((n, k) => {
                out.set([rows[width * n], rows[width * n + 1]], 2 * k);
                if (rows[width * n + 2] > top) top = rows[width * n + 2];
            });
            return Buffer.from(out.buffer).toString('base64');
        });
        if (top === -Infinity) continue;
        const { reach, quality, probed } = await page.evaluate(async ({ view, points }) => {
            const t = window.trailView, decode = text => new Float64Array(Uint8Array.from(atob(text), c => c.charCodeAt(0)).buffer);
            t.view = { ...t.view, ...view };
            await t.nextFrame();
            await t.nextFrame();
            // The terrain only: the plants, the shadows and the still view's frames change no probe.
            for (let i = 0; i < 3000 && !(t.terrain.idle && t.terrain.pages.idle); i++) await t.nextFrame();
            await t.nextFrame();
            const read = await Promise.all(points.map((text, s) => t.terrain.probeAt(decode(text), s === 1)));
            return {
                reach: t.terrain.pages.maskReach,
                quality: t.terrain.quality,
                probed: read.map(values => {
                    const bytes = new Uint8Array(values.buffer);
                    let text = '';
                    for (let i = 0; i < bytes.length; i += 32768) text += String.fromCharCode.apply(null, bytes.subarray(i, i + 32768));
                    return btoa(text);
                }),
            };
        }, { view: { lat: latOf(ey), lon: lonOf(ex), heading: 0, pitch: -40, mode: 'fly', height: top + EYE_M }, points });
        qualities.set(quality, (qualities.get(quality) ?? 0) + 1);
        probed.forEach((text, s) => {
            const values = new Float32Array(new Uint8Array(Buffer.from(text, 'base64')).buffer), { rows, width } = sets[s];
            members[s].forEach((n, k) => {
                const metres = Math.hypot(rows[width * n] - ex, rows[width * n + 1] - ey) * unit;
                if (metres <= REACH_M && (s === 0 || metres <= reach)) results[s][n] = values[k];
            });
        });
    }
    if (errors.length) console.error('page errors', errors.slice(0, 3));
} finally {
    await browser.close();
    close();
}

/** The census line of a count from the indices of its samples in a set: as countLine of census.ts. */
function countLine(count, { rows, width }, hits) {
    const places = new Map();
    for (const i of hits) {
        const [x, y] = [rows[width * i], rows[width * i + 1]], unit = metresPerUnit(y), name = `${Math.floor(x * unit / PLACE_M)},${Math.floor(y * unit / PLACE_M)}`;
        places.set(name, (places.get(name) ?? 0) + 1);
    }
    return JSON.stringify({ count, samples: hits.length, places: [...places.values()].filter(n => n >= 2).length });
}

// Tears and lifts are measured against the drawn mesh everywhere, also where a way owns the ground.
const [faces, water] = sets, tears = [], lifts = [], wet = [], unknown = [0, 0];
for (let i = 0; i < faces.rows.length / 4; i++) {
    if (faces.rows[4 * i + 3] >= DECK_BIT) continue;
    const drawn = results[0][i];
    if (Number.isNaN(drawn)) { unknown[0]++; continue; }
    const over = faces.rows[4 * i + 2] - drawn;
    if (over < -TEAR_M) tears.push(i);
    if (over > LIFT_M) lifts.push(i);
}
for (let i = 0; i < water.rows.length / 3; i++) {
    if (Number.isNaN(results[1][i])) unknown[1]++;
    else if (results[1][i] === 0) wet.push(i);
}
console.log(countLine('tears', faces, tears));
console.log(countLine('lifts', faces, lifts));
console.log(countLine('plants-in-water', water, wet));
const byQuality = [...qualities].sort(([a], [b]) => a - b).map(([q, n]) => `quality ${q}: ${n}`).join(', ');
console.error(`${((Date.now() - start) / 1000).toFixed(0)} s, ${squares.size} squares (${byQuality}); unknown: ${unknown[0]} of ${faces.rows.length / 4} face samples, ${unknown[1]} of ${water.rows.length / 3} water samples`);
