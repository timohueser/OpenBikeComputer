import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { stampBundle } from '../../../../vite/stamp-bundle';

const ROOT = fileURLToPath(new URL('../../../../../../', import.meta.url));

/**
 * The files that `stamp_sources` of host/obc-data-steps/src/planner.rs digests: the TypeScript of
 * src/lib/trailview/ but its tests and packages, the JSON of data/trailview/, and the npm packages
 * through package-lock.json.
 */
const digested = (file: string) =>
    (/^builder\/web\/src\/lib\/trailview\/.+\.ts$/.test(file) && !file.endsWith('.test.ts') && !file.includes('/pkg/')) ||
    /^data\/trailview\/.+\.json$/.test(file) ||
    file.startsWith('builder/web/node_modules/');

describe('stamp sources', () => {
    it('hold every module of the stamp bake', async () => {
        const { modules } = await stampBundle();
        expect(modules.length).toBeGreaterThan(10);
        const outside = modules.map(id => (id.startsWith(ROOT) ? id.slice(ROOT.length) : id)).filter(file => !digested(file));
        expect(outside).toEqual([]);
    }, 30_000);
});
