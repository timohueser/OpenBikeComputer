// The model review page: each breed's models in the engine's light, near at tier 0 and close up at tier 1, with the
// triangles and vertices of each tier against the budget, and the models of another build beside them.
//
//   node builder/web/tests/browser/trailview/models.mjs [--out DIR] [--before DIR] [--only a,b] [SERVED]
//
// The page must be built (`box.sh` or `npm run build:trailview`), from the same checkout. SERVED are the data
// options of shoot.mjs (--tiles, --ground, --features); the place is a flat meadow of the Engelberg box. --before names
// the folder of another run: its `<breed>-near.png` stands beside each breed (today's model, for a change of models).
// Writes DIR/index.html and one PNG per breed and tier; the showroom of dressing.ts stands the animals.

import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { bakeModels } from '../../../vite/model-files.ts';
import { decodeModels } from '../../../src/lib/trailview/models/model-file.ts';

const { values } = parseArgs({
    options: {
        out: { type: 'string', default: new URL('../../../../../.artifacts/trailview-models', import.meta.url).pathname },
        before: { type: 'string' },
        only: { type: 'string' },
        tiles: { type: 'string' },
        ground: { type: 'string' },
        features: { type: 'string' },
    },
});
/** The showroom's middle, the eye's heading toward it, and the page; SHOW_SPACING of dressing.ts. */
const PLACE = [46.820416, 8.403045], HEADING = 160, WIDTH = 1600, HEIGHT = 640, SPACING = 2.3;
const { baked } = await bakeModels();
const BUDGET = baked.budget.animal;
const only = values.only?.split(',');
const breeds = Object.entries(baked.models.breeds).filter(([name]) => !only || only.includes(name));
mkdirSync(values.out, { recursive: true });

// The eye stands far enough before the row to see all of it.
const views = breeds.flatMap(([name, { height, coats, poses }]) => {
    const distance = Math.max(6, (0.5 * coats * poses * SPACING * height) / 1.1), a = (HEADING * Math.PI) / 180;
    const lat = PLACE[0] - (distance * Math.cos(a)) / 111320, lon = PLACE[1] - (distance * Math.sin(a)) / (111320 * Math.cos((PLACE[0] * Math.PI) / 180));
    const view = (tier) => `?lat=${lat.toFixed(7)}&lon=${lon.toFixed(7)}&heading=${HEADING}&pitch=-6&eye=1.6&date=2026-08-15&time=15:00&weather=sunny&showroom=${PLACE.join(',')},${HEADING},${name},${tier}`;
    return [{ name: `${name}-near`, view: view(0) }, { name: `${name}-tier1`, view: view(1) }];
});
writeFileSync(join(values.out, 'views.json'), JSON.stringify(views));
const served = ['tiles', 'ground', 'features'].flatMap(key => (values[key] ? [`--${key}`, values[key]] : []));
const shot = spawnSync(process.execPath, [new URL('./shoot.mjs', import.meta.url).pathname, '--gpu', '--views', join(values.out, 'views.json'), '--out', values.out, '--width', `${WIDTH}`, '--height', `${HEIGHT}`, ...served], { stdio: 'inherit' });
if (shot.status !== 0) process.exit(shot.status ?? 1);

const rows = [];
for (const [name, { height, coats, poses, file }] of breeds) {
    const models = await decodeModels(baked.files.get(file));
    const counts = models.map(({ tiers }, k) => `<tr><td>${k}: coat ${Math.floor(k / poses)}, pose ${k % poses}</td>${tiers.map(({ vertices, index }, t) => {
        const tris = index.length / 3, verts = vertices.length / 16, over = tris > BUDGET[t][0] || verts > BUDGET[t][1];
        return `<td class="${over ? 'over' : ''}">${tris} / ${BUDGET[t][0]}</td><td class="${over ? 'over' : ''}">${verts} / ${BUDGET[t][1]}</td>`;
    }).join('')}</tr>`).join('');
    const before = values.before && join(values.before, `${name}-near.png`);
    if (before && existsSync(before)) copyFileSync(before, join(values.out, `${name}-before.png`));
    rows.push(`<section><h2>${name}</h2><p>${height} m to the back, ${coats} coats × ${poses} poses, ${baked.files.get(file).length} bytes</p>
${before && existsSync(before) ? `<figure><img src="${name}-before.png"><figcaption>Before</figcaption></figure>` : ''}
<figure><img src="${name}-near.png"><figcaption>Tier 0, to 150 m</figcaption></figure>
<figure><img src="${name}-tier1.png"><figcaption>Tier 1, to 1,500 m, drawn close</figcaption></figure>
<table><tr><th>Model</th><th>Tier 0 triangles</th><th>Tier 0 vertices</th><th>Tier 1 triangles</th><th>Tier 1 vertices</th></tr>${counts}</table></section>`);
}
writeFileSync(join(values.out, 'index.html'), `<!doctype html><meta charset="utf-8"><title>TrailView models</title>
<style>body{font:14px system-ui;margin:16px;max-width:1600px}img{width:100%}figure{margin:8px 0}td,th{padding:2px 8px;text-align:right}.over{color:#c00;font-weight:600}</style>
<h1>TrailView models</h1>${rows.join('\n')}`);
console.log(join(values.out, 'index.html'));
