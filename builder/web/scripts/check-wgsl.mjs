// Validates every WGSL file of TrailView with naga, composed with its includes, and the declarations that the
// entry points of a file reach, as the engine makes its modules.
//
//   npm run check:wgsl          # cargo install naga-cli --version 30.0.1 --locked
import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { CLIP_DISTANCES, include, MOUTH, reached } from '../src/lib/trailview/gpu/include.ts';

const root = new URL('../src/lib/trailview/', import.meta.url).pathname;
const files = readdirSync(root, { recursive: true }).filter(file => file.endsWith('.wgsl')).sort();
let failed = 0;
const check = (file, swap = {}) => {
    const composed = include(file, path => readFileSync(root + (swap[path] ?? path), 'utf8')), name = `${file}${swap === CLIP_DISTANCES ? ' with clip distances' : swap === MOUTH ? ' with the mouth' : ''}`;
    for (const [label, input] of [[name, composed], ...(/@(vertex|fragment|compute)\b/.test(composed) ? [[`${name}, reached`, reached(composed)]] : [])]) {
        process.stdout.write(`${label}: `);
        try {
            execFileSync('naga', ['--stdin-file-path', file], { input, stdio: ['pipe', 'inherit', 'inherit'] });
        } catch {
            failed++;
        }
    }
};
// A file that takes another's place checks in the composition that swaps it in.
const swapped = new Set(Object.values(MOUTH));
files.filter(file => !swapped.has(file)).forEach(file => check(file));
check('plan/piece.wgsl', MOUTH);
check('terrain/terrain.wgsl', CLIP_DISTANCES);
check('terrain/water.wgsl', CLIP_DISTANCES);
process.exit(failed ? 1 : 0);
