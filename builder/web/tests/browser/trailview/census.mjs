// The GPU half of the TrailView fault census: reads the samples that scripts/road-census.ts wrote to
// SAMPLES_DIR, probes the drawn terrain and the mask at them, and prints the tears, the lifts, the gaps in owned
// ground and the plants in water over the fixture, one JSON line each, as {"count":"tears","samples":767,"places":86}.
//
//   npm run pretest:trailview                    # build against the fixture
//   flock /tmp/trailview-gpu.lock node trailview/census.mjs [--gpu] [--ground DIR] SAMPLES_DIR
//
// Per square of CELL_M metres, an eye EYE_M metres over the square's highest sample, at its middle, draws the
// terrain all round within REACH_M metres, at the finest level near it. Once the terrain and the corridors are
// idle, the probe reads the mesh as the terrain draws it (its clamp included) and `owned` at the square's face
// samples, kept_off at its water samples, and `owned` on a grid of GAP_STEP_M metres over the square. A sample
// farther from the eye than REACH_M, or a water sample past the mask pool's reach, stays unknown, as does a sample
// that no page knows. --gpu draws on the machine's GPU, else SwiftShader: the census then takes hours. Prints to
// stderr the time, the squares per terrain quality (a quality over 0 coarsens the levels) and the unknown samples.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import config from './fixture/config.json' with { type: 'json' };
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
/** The grid of the gaps in owned ground, in metres; the census leaves out MARGIN_M at the fixture's edges, as census.ts does. */
const GAP_STEP_M = 0.25, MARGIN_M = 70;

/** As metresPerUnit of bins.ts, and worldPixel of mercator.ts in world units. */
const metresPerUnit = y => 2 * Math.PI * 6_371_000 / UNITS / Math.cosh(Math.PI * (1 - 2 * y / UNITS));
const lonOf = x => x / UNITS * 360 - 180, latOf = y => Math.atan(Math.sinh(Math.PI * (1 - 2 * y / UNITS))) * 180 / Math.PI;
const xOf = lon => (lon + 180) / 360 * UNITS, yOf = lat => (0.5 - Math.asinh(Math.tan(lat * Math.PI / 180)) / (2 * Math.PI)) * UNITS;
const [west, east, north, south] = [xOf(config.bounds[0]), xOf(config.bounds[2]), yOf(config.bounds[3]), yOf(config.bounds[1])];
const rowsOf = name => {
    const bytes = readFileSync(join(positionals[0], `${name}.bin`));
    return new Float64Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 8);
};
const sets = [{ rows: rowsOf('faces'), width: 4 }, { rows: rowsOf('water'), width: 3 }];
const results = sets.map(({ rows, width }) => new Float32Array(rows.length / width).fill(NaN)), owned = new Float32Array(sets[0].rows.length / 4);
/** The points of owned ground that no near face covers, world x, y pairs. */
const gaps = [];

// The samples per set, by their square of CELL_M metres, in world units at the first sample's latitude.
const squares = new Map(), cell = CELL_M / metresPerUnit(sets[0].rows[1]);
sets.forEach(({ rows, width }, s) => {
    for (let i = 0; i < rows.length / width; i++) {
        const name = `${Math.floor(rows[width * i] / cell)},${Math.floor(rows[width * i + 1] / cell)}`;
        (squares.get(name) ?? squares.set(name, [[], []]).get(name))[s].push(i);
    }
});

/**
 * In the page: the points of a grid of `step` world units over a box [x0, y0, x1, y1] where a way owns the ground and
 * no near face of a corridor that the page holds covers it in plan, its skirt included. A face decodes as census.ts
 * reads a piece's vertices; a piece of level `child` holds a corridor's faces, a finer one its details.
 */
async function ownedGaps({ box: [x0, y0, x1, y1], step, child }) {
    const t = window.trailView, faces = [], index = new Map(), CELL = 256;
    for (const mesh of t.terrain.roads.meshes.values()) for (const piece of mesh.pieces) {
        if (piece.level !== child) continue;
        const steps = new Int16Array(piece.vertices), words = new Uint32Array(piece.vertices);
        const x = v => piece.x + (steps[8 * v] * 4 + ((words[4 * v + 2] >>> 28) & 3)) / 12, y = v => piece.y + (steps[8 * v + 1] * 4 + (words[4 * v + 2] >>> 30)) / 12;
        for (let k = piece.near.first; k < piece.near.first + piece.near.count; k += 3) {
            const f = [0, 1, 2].flatMap(i => [x(piece.indices[k + i]), y(piece.indices[k + i])]);
            const [lx, hx, ly, hy] = [Math.min(f[0], f[2], f[4]), Math.max(f[0], f[2], f[4]), Math.min(f[1], f[3], f[5]), Math.max(f[1], f[3], f[5])];
            if (hx < x0 || lx > x1 || hy < y0 || ly > y1) continue;
            for (let i = Math.floor(lx / CELL); i <= Math.floor(hx / CELL); i++) for (let j = Math.floor(ly / CELL); j <= Math.floor(hy / CELL); j++) {
                const name = i * 65536 + j;
                (index.get(name) ?? index.set(name, []).get(name)).push(faces.length);
            }
            faces.push(f);
        }
    }
    const inside = ([ax, ay, bx, by, cx, cy], px, py) => {
        const d0 = (px - bx) * (ay - by) - (ax - bx) * (py - by), d1 = (px - cx) * (by - cy) - (bx - cx) * (py - cy), d2 = (px - ax) * (cy - ay) - (cx - ax) * (py - ay);
        return (d0 <= 0 && d1 <= 0 && d2 <= 0) || (d0 >= 0 && d1 >= 0 && d2 >= 0);
    };
    const columns = Math.floor((x1 - x0) / step), rows = Math.floor((y1 - y0) / step), points = new Float64Array(2 * columns * rows);
    for (let j = 0; j < rows; j++) for (let i = 0; i < columns; i++) points.set([x0 + (i + 0.5) * step, y0 + (j + 0.5) * step], 2 * (j * columns + i));
    const probed = await t.terrain.probeAt(points, 'owned'), out = [];
    for (let n = 0; n < probed.length; n++) {
        if (probed[n] !== 1) continue;
        const [px, py] = [points[2 * n], points[2 * n + 1]], list = index.get(Math.floor(px / CELL) * 65536 + Math.floor(py / CELL)) ?? [];
        if (!list.some(f => inside(faces[f], px, py))) out.push(px, py);
    }
    return out;
}

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
    await page.addScriptTag({ content: `window.ownedGaps = ${ownedGaps.toString()};` });
    for (const [name, members] of squares) {
        const [i, j] = name.split(',').map(Number), [ex, ey] = [(i + 0.5) * cell, (j + 0.5) * cell], unit = metresPerUnit(ey), margin = MARGIN_M / unit;
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
        const box = [Math.max(i * cell, west + margin), Math.max(j * cell, north + margin), Math.min((i + 1) * cell, east - margin), Math.min((j + 1) * cell, south - margin)];
        const { reach, quality, probed, holes } = await page.evaluate(async ({ view, points, box, step }) => {
            const t = window.trailView, decode = text => new Float64Array(Uint8Array.from(atob(text), c => c.charCodeAt(0)).buffer);
            t.view = { ...t.view, ...view };
            await t.nextFrame();
            await t.nextFrame();
            // The terrain and the corridors only: the plants, the shadows and the still view's frames change no probe.
            for (let i = 0; i < 3000 && !(t.terrain.idle && t.terrain.pages.idle && !t.terrain.roadsLoading); i++) await t.nextFrame();
            await t.nextFrame();
            const [faces, water] = points.map(decode);
            const read = await Promise.all([t.terrain.probeAt(faces, 'clamped'), t.terrain.probeAt(water, 'kept'), t.terrain.probeAt(faces, 'owned')]);
            const holes = box[0] < box[2] && box[1] < box[3] ? await window.ownedGaps({ box, step, child: 16 }) : [];
            return {
                reach: t.terrain.pages.maskReach,
                quality: t.terrain.quality,
                probed: read.map(values => {
                    const bytes = new Uint8Array(values.buffer);
                    let text = '';
                    for (let i = 0; i < bytes.length; i += 32768) text += String.fromCharCode.apply(null, bytes.subarray(i, i + 32768));
                    return btoa(text);
                }),
                holes,
            };
        }, { view: { lat: latOf(ey), lon: lonOf(ex), heading: 0, pitch: -40, mode: 'fly', height: top + EYE_M }, points, box, step: GAP_STEP_M / unit });
        qualities.set(quality, (qualities.get(quality) ?? 0) + 1);
        gaps.push(...holes);
        probed.forEach((text, s) => {
            // The probes: the drawn mesh at the face samples, kept_off at the water samples, `owned` at the face samples.
            const set = s === 2 ? 0 : s, values = new Float32Array(new Uint8Array(Buffer.from(text, 'base64')).buffer), { rows, width } = sets[set];
            members[set].forEach((n, k) => {
                const metres = Math.hypot(rows[width * n] - ex, rows[width * n + 1] - ey) * unit;
                if (s === 2) owned[n] = values[k];
                else if (metres <= REACH_M && (s === 0 || metres <= reach)) results[s][n] = values[k];
            });
        });
    }
    if (errors.length) console.error('page errors', errors.slice(0, 3));
} finally {
    await browser.close();
    close();
}

/** The census line of a count from world x, y pairs: as countLine of census.ts. */
function countLine(count, points) {
    const places = new Map();
    for (let i = 0; i < points.length; i += 2) {
        const unit = metresPerUnit(points[i + 1]), name = `${Math.floor(points[i] * unit / PLACE_M)},${Math.floor(points[i + 1] * unit / PLACE_M)}`;
        places.set(name, (places.get(name) ?? 0) + 1);
    }
    return JSON.stringify({ count, samples: points.length / 2, places: [...places.values()].filter(n => n >= 2).length });
}

// Tears count against the mesh as the terrain draws it, also where a way owns the ground: its clamp lies under the
// corridor there. Lifts count only outside owned ground, where the terrain does not lie lower by design.
const [faces, water] = sets, tears = [], lifts = [], wet = [], unknown = [0, 0];
for (let i = 0; i < faces.rows.length / 4; i++) {
    if (faces.rows[4 * i + 3] >= DECK_BIT) continue;
    const drawn = results[0][i], at = [faces.rows[4 * i], faces.rows[4 * i + 1]];
    if (Number.isNaN(drawn)) { unknown[0]++; continue; }
    const over = faces.rows[4 * i + 2] - drawn;
    if (over < -TEAR_M) tears.push(...at);
    if (over > LIFT_M && owned[i] !== 1) lifts.push(...at);
}
for (let i = 0; i < water.rows.length / 3; i++) {
    if (Number.isNaN(results[1][i])) unknown[1]++;
    else if (results[1][i] === 0) wet.push(water.rows[3 * i], water.rows[3 * i + 1]);
}
console.log(countLine('tears', tears));
console.log(countLine('lifts', lifts));
console.log(countLine('owned-gaps', gaps));
console.log(countLine('plants-in-water', wet));
const byQuality = [...qualities].sort(([a], [b]) => a - b).map(([q, n]) => `quality ${q}: ${n}`).join(', ');
console.error(`${((Date.now() - start) / 1000).toFixed(0)} s, ${squares.size} squares (${byQuality}); unknown: ${unknown[0]} of ${faces.rows.length / 4} face samples, ${unknown[1]} of ${water.rows.length / 3} water samples`);
