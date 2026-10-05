import type { Archive } from './layers/data-layer';

export type Bounds = [number, number, number, number];

/**
 * The planner config: the `active` entry of the planner catalogue in specs/planner-release.md, which a
 * planner build carries as `VITE_PLANNER_CONFIG`. A local preview gives the same fields as root-relative
 * paths, so a preview opened as localhost or 127.0.0.1 stays on one origin.
 */
export interface PlannerConfig {
    name: string;
    bounds: Bounds;
    /** TileJSON URLs; a local preview gives `pmtiles://` archive paths. */
    basemap: string;
    places: string;
    overlays: string;
    /** A Terrarium WebP tile template. */
    terrain: string;
    terrain_attribution: string;
    glyphs: string;
    sprites: string;
    routing: string;
    search: string;
    /** The route catalog of specs/route-catalog.md: a cell template with `{cell}`, or the region file. */
    routes?: string;
    /** The data layer archives of the region, by name. */
    layers: Partial<Record<Archive, string>>;
}

const URLS = ['basemap', 'places', 'overlays', 'terrain', 'glyphs', 'sprites', 'routing', 'search'] as const;

/**
 * `value` as a URL: an absolute URL as it is, or a root-relative path, also inside `pmtiles://`, resolved
 * against `base`. Undefined when `value` is neither.
 */
export function configUrl(value: unknown, base?: string): string | undefined {
    if (typeof value !== 'string' || !/^([a-z]+:\/\/|\/)/.test(value)) return undefined;
    const [, scheme = '', path] = /^(pmtiles:\/\/)?(\/.*)$/.exec(value) ?? [];
    if (!path || !base) return value;
    // The contour workers resolve no relative URL; URL encodes the template braces that MapLibre expands.
    return scheme + new URL(path, base).href.replace(/%7B/g, '{').replace(/%7D/g, '}');
}

/**
 * The config in `text`, with its paths resolved against `base`, the page. A config that the planner
 * cannot use throws, so the build that carries it fails.
 */
export function plannerConfig(text: string | undefined, base?: string): PlannerConfig {
    if (!text) throw new Error('Set VITE_PLANNER_CONFIG to the planner config; see the planner README.');
    const config = JSON.parse(text);
    const bounds = config.bounds;
    const urls = Object.fromEntries(URLS.map((key) => [key, configUrl(config[key], base)]));
    const routes = config.routes === undefined ? undefined : configUrl(config.routes, base);
    const layers = Object.fromEntries(Object.entries(config.layers ?? {}).map(([name, value]) => [name, configUrl(value, base)]));
    const invalid = [
        ...URLS.filter((key) => !urls[key]),
        ...(config.routes === undefined || routes ? [] : ['routes']),
        ...(typeof config.name === 'string' && config.name ? [] : ['name']),
        ...(typeof config.terrain_attribution === 'string' ? [] : ['terrain_attribution']),
        ...(config.layers && typeof config.layers === 'object' && Object.values(layers).every(Boolean) ? [] : ['layers']),
        ...(Array.isArray(bounds) && bounds.length === 4 && bounds.every(Number.isFinite) && bounds[0] >= -180 && bounds[2] <= 180
            && bounds[1] >= -85 && bounds[3] <= 85 && bounds[0] < bounds[2] && bounds[1] < bounds[3] ? [] : ['bounds']),
    ];
    if (invalid.length) throw new Error(`The planner config has invalid fields: ${invalid.join(', ')}.`);
    return { ...config, ...urls, routes, layers };
}
