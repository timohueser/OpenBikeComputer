import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { partsSources, stampBundle, versionOf } from '../../../../vite/stamp-bundle';

const ROOT = fileURLToPath(new URL('../../../../../../', import.meta.url));

/**
 * The files that `stamp_sources` of host/obc-data-steps/src/planner.rs digests: the TypeScript of
 * src/lib/trailview/ but its tests and packages, the JSON of data/trailview/, the parts bridge through its
 * sources, and the npm packages through package-lock.json.
 */
const digested = (file: string) =>
    (/^builder\/web\/src\/lib\/trailview\/.+\.ts$/.test(file) && !file.endsWith('.test.ts') && !file.includes('/pkg/')) ||
    /^data\/trailview\/.+\.json$/.test(file) ||
    file.startsWith('builder/web/src/lib/trailview/structures/pkg/') ||
    file.startsWith('builder/web/node_modules/');

describe('stamp sources', () => {
    it('hold every module of the stamp bake', async () => {
        const { modules } = await stampBundle();
        expect(modules.length).toBeGreaterThan(10);
        const outside = modules.map(id => (id.startsWith(ROOT) ? id.slice(ROOT.length) : id)).filter(file => !digested(file));
        expect(outside).toEqual([]);
    }, 30_000);

    it('take the sources of the parts bridge for its bytes, and refuse a package built from other sources', () => {
        const code = (bytes: string) => `export default "data:application/wasm;base64,${bytes}";`, sources = partsSources();
        expect(readFileSync(new URL('../structures/pkg/sources.sha256', import.meta.url), 'utf8').trim()).toBe(sources);
        expect(versionOf(code('AGFzbQ=='), `${sources}\n`, sources)).toBe(versionOf(code('AGFzbQE='), sources, sources));
        expect(() => versionOf(code('AGFzbQ=='), '0'.repeat(64), sources)).toThrow(/names other sources/);
        expect(() => versionOf(code('AGFzbQ=='), undefined, sources)).toThrow(/is missing/);
        expect(() => versionOf(code('AGFzbQ==') + code('AGFzbQE='), sources, sources)).toThrow(/2 inlined WASM modules/);
    });
});
