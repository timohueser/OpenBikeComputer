import { include } from './include';

const SOURCES = import.meta.glob('../**/*.wgsl', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

/** The engine's WGSL file at a path such as `terrain/terrain.wgsl`, with its includes. */
export const wgsl = (path: string) => include(path, file => SOURCES[`../${file}`]);
