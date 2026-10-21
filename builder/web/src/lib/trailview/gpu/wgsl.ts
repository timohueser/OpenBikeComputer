import { include } from './include';

const SOURCES = import.meta.glob('../**/*.wgsl', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

/** The engine's WGSL file at a path such as `terrain/terrain.wgsl`, with its includes; `swap` reads another file for an include. */
export const wgsl = (path: string, swap: Record<string, string> = {}) => include(path, file => SOURCES[`../${swap[file] ?? file}`]);
