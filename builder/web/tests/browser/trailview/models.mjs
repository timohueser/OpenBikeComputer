// The model review page: each breed's models in the engine's light, near at tier 0 and close up at tier 1; or with
// `--kind trees` each tree species at tiers 0, 1 and 2 side by side, in summer and in winter, 4 species a section. Each with
// the triangles and vertices of each tier against the budget, and the models of another build beside them.
//
//   node builder/web/tests/browser/trailview/models.mjs [--kind animals|trees] [--out DIR] [--before DIR] [--only a,b] [SERVED]
//
// The page must be built (`box.sh` or `npm run build:trailview`), from the same checkout. SERVED are the data
// options of shoot.mjs (--tiles, --ground, --features); the place is a flat meadow of the Engelberg box. --before names
// the folder of another run: its picture of the same name stands beside each one (today's model, for a change of
// models). Writes DIR/index.html and the PNGs; the showroom of dressing.ts stands the models, its trees in their own
// colours, as they have no ground to take the satellite's hue from.

import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { bakeModels } from '../../../vite/model-files.ts';
import { decodeModels } from '../../../src/lib/trailview/models/model-file.ts';

const { values } = parseArgs({
    options: {
        kind: { type: 'string', default: 'animals' },
        out: { type: 'string', default: new URL('../../../../../.artifacts/trailview-models', import.meta.url).pathname },
        before: { type: 'string' },
        only: { type: 'string' },
        tiles: { type: 'string' },
        ground: { type: 'string' },
        features: { type: 'string' },
    },
});
/** The showroom's middle, the eye's heading toward it, and the page; SHOW_SPACING and SHOW_TREE_M of dressing.ts. */
const PLACE = [46.820416, 8.403045], HEADING = 160, WIDTH = 1600, HEIGHT = 640, SPACING = 2.3, TREE_M = 10;
/** Tree species a section of the page, the dates of the views (summer and winter), and the size of a tree view. */
const GROUP = 4, DATES = { summer: '2026-08-15', winter: '2026-01-20' }, TREE_VIEW = [1200, 800];
const { baked } = await bakeModels();
const only = values.only?.split(',');
const trees = values.kind === 'trees';
mkdirSync(values.out, { recursive: true });

/** [lat, lon] moved d metres toward HEADING. */
const ahead = ([lat, lon], d) => [lat + (d * Math.cos((HEADING * Math.PI) / 180)) / 111320, lon + (d * Math.sin((HEADING * Math.PI) / 180)) / (111320 * Math.cos((lat * Math.PI) / 180))];

/**
 * A view of the showroom with `entries` in a row `width` metres wide, the eye far enough before it to see all of it
 * (`tan` is the tangent of half the view's width). The animals stand at PLACE; the eye of the taller trees stands
 * there, as the meadow is wider ahead.
 */
function view(entries, width, { tier = 0, date = DATES.summer, eye = 1.6, pitch = -6, tan = 1.1, trees = false } = {}) {
    const distance = Math.max(6, (0.5 * width) / tan), [from, row] = trees ? [PLACE, ahead(PLACE, distance)] : [ahead(PLACE, -distance), PLACE];
    return `?lat=${from[0].toFixed(7)}&lon=${from[1].toFixed(7)}&heading=${HEADING}&pitch=${pitch}&eye=${eye}&date=${date}&time=15:00&weather=sunny&showroom=${row.map(v => v.toFixed(7)).join(',')},${HEADING},${entries.join('|')},${tier}`;
}

const breeds = trees ? [] : Object.entries(baked.models.breeds).filter(([name]) => !only || only.includes(name));
const species = trees ? Object.keys(baked.models.trees).filter(id => !only || only.includes(id)) : [];
// A tree's room in the row: SHOW_TREE_M, or its crown's width where that is wider, so wide crowns do not overlap.
const rooms = Object.fromEntries(await Promise.all(species.map(async id => [id, TREE_M * Math.max(1, 2.2 * (await decodeModels(baked.files.get(baked.models.trees[id].summer)))[0].shape.spread)])));
const groups = Array.from({ length: Math.ceil(species.length / GROUP) }, (_, g) => species.slice(GROUP * g, GROUP * (g + 1)));
// A tree view stands one species' tiers: a row of 12 trees would be too small to judge in one picture.
const views = trees
    ? species.flatMap(id => Object.entries(DATES).map(([season, date]) => ({
        name: `${id}-${season}`, view: view([0, 1, 2].map(tier => `tree.${id}.${tier}.${Math.round(rooms[id])}`), 3.3 * rooms[id], { date, eye: 0.3 * TREE_M, pitch: 7, tan: (0.577 * TREE_VIEW[0]) / TREE_VIEW[1], trees: true }),
    })))
    : breeds.flatMap(([name, { height, coats, poses }]) => [0, 1].map(tier => ({ name: `${name}-${tier ? 'tier1' : 'near'}`, view: view([name], coats * poses * SPACING * height, { tier }) })));
writeFileSync(join(values.out, 'views.json'), JSON.stringify(views));
const served = ['tiles', 'ground', 'features'].flatMap(key => (values[key] ? [`--${key}`, values[key]] : []));
const shot = spawnSync(process.execPath, [new URL('./shoot.mjs', import.meta.url).pathname, '--gpu', '--views', join(values.out, 'views.json'), '--out', values.out, '--width', `${trees ? TREE_VIEW[0] : WIDTH}`, '--height', `${trees ? TREE_VIEW[1] : HEIGHT}`, ...served], { stdio: 'inherit' });
if (shot.status !== 0) process.exit(shot.status ?? 1);

/** A picture of this run, and the one of the same name from --before beside it. */
function figure(name, caption) {
    const before = values.before && join(values.before, `${name}.png`), has = before && existsSync(before);
    if (has) copyFileSync(before, join(values.out, `${name}-before.png`));
    return `${has ? `<figure><img src="${name}-before.png"><figcaption>Before: ${caption}</figcaption></figure>` : ''}<figure><img src="${name}.png"><figcaption>${caption}</figcaption></figure>`;
}

/** Table cells of the tiers of a model: triangles and vertices against the budget of its kind. */
const cells = (tiers, budget) => tiers.map(({ vertices, index }, t) => {
    const tris = index.length / 3, verts = vertices.length / 16, over = tris > budget[t][0] || verts > budget[t][1];
    return `<td class="${over ? 'over' : ''}">${tris} / ${budget[t][0]}</td><td class="${over ? 'over' : ''}">${verts} / ${budget[t][1]}</td>`;
}).join('');

const rows = [];
for (const group of groups) {
    const counts = [];
    for (const id of group) {
        const { summer, crowns } = baked.models.trees[id];
        for (const file of [summer, ...crowns.filter(Boolean)]) {
            const [{ tiers }] = await decodeModels(baked.files.get(file));
            counts.push(`<tr><td>${file}</td>${cells(tiers, baked.budget.tree)}</tr>`);
        }
    }
    rows.push(`<section><h2>${group.join(', ')}</h2><p>Each species at tiers 0, 1 and 2 from the left, ${TREE_M} m tall.</p>
${Object.keys(DATES).map(season => `<div class="trees">${group.map(id => figure(`${id}-${season}`, `${id}, ${season}`)).join('')}</div>`).join('\n')}
<table><tr><th>File</th>${[0, 1, 2].map(t => `<th>Tier ${t} triangles</th><th>Tier ${t} vertices</th>`).join('')}</tr>${counts.join('')}</table></section>`);
}
for (const [name, { height, coats, poses, file }] of breeds) {
    const models = await decodeModels(baked.files.get(file));
    const counts = models.map(({ tiers }, k) => `<tr><td>${k}: coat ${Math.floor(k / poses)}, pose ${k % poses}</td>${cells(tiers, baked.budget.animal)}</tr>`).join('');
    rows.push(`<section><h2>${name}</h2><p>${height} m to the back, ${coats} coats × ${poses} poses, ${baked.files.get(file).length} bytes</p>
${figure(`${name}-near`, 'Tier 0, to 150 m')}
<figure><img src="${name}-tier1.png"><figcaption>Tier 1, to 1,500 m, drawn close</figcaption></figure>
<table><tr><th>Model</th><th>Tier 0 triangles</th><th>Tier 0 vertices</th><th>Tier 1 triangles</th><th>Tier 1 vertices</th></tr>${counts}</table></section>`);
}
writeFileSync(join(values.out, 'index.html'), `<!doctype html><meta charset="utf-8"><title>TrailView models</title>
<style>body{font:14px system-ui;margin:16px;max-width:1600px}img{width:100%}figure{margin:8px 0}td,th{padding:2px 8px;text-align:right}.trees{display:grid;grid-template-columns:repeat(4,1fr);gap:8px}.over{color:#c00;font-weight:600}</style>
<h1>TrailView models</h1>${rows.join('\n')}`);
console.log(join(values.out, 'index.html'));
