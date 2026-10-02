import {execFileSync} from 'node:child_process';
import {cp, mkdir, readFile, writeFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const output = process.argv[2] && path.resolve(process.argv[2]);
if (!output) throw new Error('Usage: node apps/planner-native/build.mjs OUTPUT');
const app = path.join(root, 'builder/app');
execFileSync(process.execPath, [path.join(app,'node_modules/vite/bin/vite.js'), 'build', '--mode', 'planner', '--outDir', output], {
  cwd: app, stdio: 'inherit', env: {...process.env,
    VITE_PLANNER_TILEJSON_URL: '', VITE_PLANNER_PMTILES_URL: '/maps/basemap.pmtiles',
    VITE_PLANNER_DEM_URL: '/maps/terrain/{z}/{x}/{y}.webp', VITE_PLANNER_SEARCH_URL: '',
    VITE_PLANNER_GLYPHS_URL: '/maps/assets/fonts/{fontstack}/{range}.pbf',
    VITE_PLANNER_SPRITES_URL: '/maps/assets/sprites/v4', VITE_PLANNER_ROUTING_URL: '/routing',
    VITE_PLANNER_SEARCH_REGIONS: 'baden-wuerttemberg', VITE_SITE_BASE: '/',
  },
});
execFileSync(process.execPath, [path.join(app,'vite/build-terrain-worker.mjs'),path.join(output,'dem-worker.js')], {stdio:'inherit'});
await mkdir(path.join(output,'brand'), {recursive:true});
for (const name of ['app-icon.svg','signpost.svg']) await cp(path.join(app,'public/brand',name),path.join(output,'brand',name));
const xml = await readFile(path.join(root,'fixtures/sources/route-import/komoot-schwarzwald.gpx'),'utf8');
await writeFile(path.join(output,'sample.json'),JSON.stringify({coordinates:[...xml.matchAll(/<trkpt lat="([^"]+)" lon="([^"]+)"/g)].map(m=>[+m[2],+m[1]])}));
await cp(path.join(output,'planner.html'),path.join(output,'index.html'));
console.log(JSON.stringify({output}));
