import { include, reached } from './include';

const SOURCES = import.meta.glob('../**/*.wgsl', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;
const composed = new Map<string, string>();

/**
 * The engine's WGSL file at a path such as `terrain/terrain.wgsl`, with its includes, as the declarations
 * that its entry points reach; `swap` reads another file for an include.
 */
export function wgsl(path: string, swap: Record<string, string> = {}): string {
    const key = `${path} ${JSON.stringify(swap)}`;
    if (!composed.has(key)) composed.set(key, reached(include(path, file => SOURCES[`../${swap[file] ?? file}`])));
    return composed.get(key)!;
}
