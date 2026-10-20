// Validates every WGSL file of TrailView with naga, composed with its includes as the engine does it.
//
//   npm run check:wgsl          # cargo install naga-cli --version 30.0.1 --locked
import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { include } from '../src/lib/trailview/gpu/include.ts';

const root = new URL('../src/lib/trailview/', import.meta.url).pathname;
const files = readdirSync(root, { recursive: true }).filter(file => file.endsWith('.wgsl')).sort();
let failed = 0;
for (const file of files) {
    process.stdout.write(`${file}: `);
    try {
        execFileSync('naga', ['--stdin-file-path', file], { input: include(file, path => readFileSync(root + path, 'utf8')), stdio: ['pipe', 'inherit', 'inherit'] });
    } catch {
        failed++;
    }
}
process.exit(failed ? 1 : 0);
