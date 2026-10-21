// Validates every WGSL file of TrailView with naga, composed with its includes as the engine does it.
//
//   npm run check:wgsl          # cargo install naga-cli --version 30.0.1 --locked
import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { CLIP_DISTANCES, include } from '../src/lib/trailview/gpu/include.ts';

const root = new URL('../src/lib/trailview/', import.meta.url).pathname;
const files = readdirSync(root, { recursive: true }).filter(file => file.endsWith('.wgsl')).sort();
let failed = 0;
const check = (file, swap = {}) => {
    process.stdout.write(`${file}${swap === CLIP_DISTANCES ? ' with clip distances' : ''}: `);
    try {
        execFileSync('naga', ['--stdin-file-path', file], { input: include(file, path => readFileSync(root + (swap[path] ?? path), 'utf8')), stdio: ['pipe', 'inherit', 'inherit'] });
    } catch {
        failed++;
    }
};
files.forEach(file => check(file));
check('terrain/terrain.wgsl', CLIP_DISTANCES);
process.exit(failed ? 1 : 0);
