import { include, reached } from './include';
import { PROTO_LINES, VARIANT } from '../proto/places';

const SOURCES = import.meta.glob('../**/*.wgsl', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;
const composed = new Map<string, string>();

/**
 * The engine's WGSL file at a path such as `terrain/terrain.wgsl`, with its includes, as the declarations
 * that its entry points reach; `swap` reads another file for an include.
 */
export function wgsl(path: string, swap: Record<string, string> = {}): string {
    const key = `${path} ${JSON.stringify(swap)}`;
    if (!composed.has(key)) {
        // PROTOTYPE: a variant's build puts its lines in the place of each `// #proto NAME` line; the structures call the ground's look in a branch.
        const read = (file: string) => (VARIANT ? SOURCES[`../${swap[file] ?? file}`].replace(/^\s*\/\/ #proto (\w+)$/gm, (_, name: string) => PROTO_LINES[name]) : SOURCES[`../${swap[file] ?? file}`]);
        composed.set(key, reached((VARIANT && path === 'structures/structures.wgsl' ? 'diagnostic(off, derivative_uniformity);\n' : '') + include(path, read)));
    }
    return composed.get(key)!;
}
