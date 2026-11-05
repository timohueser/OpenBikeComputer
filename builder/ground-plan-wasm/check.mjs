// Check 7: the WASM build gives the native digest for every request of a `requests` folder.
//
//   node builder/ground-plan-wasm/check.mjs DIR
//
// Prints `cell digest native-ms wasm-ms` per request; exits 1 on a different digest or an error.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { build_bytes, initSync } from '../web/src/lib/trailview/plan/pkg/obc_ground_plan_bridge.js';

const dir = process.argv[2];
if (!dir) throw new Error('usage: node check.mjs DIR');
initSync({ module: readFileSync(new URL('../web/src/lib/trailview/plan/pkg/obc_ground_plan_bridge_bg.wasm', import.meta.url)) });
const index = JSON.parse(readFileSync(join(dir, 'index.json'), 'utf8'));
let failed = 0;
for (const entry of index) {
    const name = entry.cell.join('-');
    try {
        const start = performance.now();
        const plan = build_bytes(readFileSync(join(dir, entry.file)));
        const ms = performance.now() - start;
        const digest = plan.digest();
        plan.free();
        const same = digest === entry.digest;
        if (!same) failed++;
        console.log(`${name} ${digest}${same ? '' : ` != native ${entry.digest}`} ${entry.native.total.toFixed(2)} ${ms.toFixed(2)}`);
    } catch (error) {
        failed++;
        console.log(`${name} error: ${error instanceof Error ? error.message : error}`);
    }
}
console.log(`${index.length - failed} of ${index.length} cells give the native digest`);
process.exit(failed ? 1 : 0);
