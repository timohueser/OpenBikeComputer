import type { Archive } from './layers/data-layer';

export type Bounds = [number, number, number, number];

/** The planner config: the `active` entry of the planner catalogue in specs/planner-release.md, or `VITE_PLANNER_CONFIG`. */
export interface PlannerConfig {
    /** The release ID; a `VITE_PLANNER_CONFIG` has none. */
    id?: string;
    name: string;
    bounds: Bounds;
    /** TileJSON URLs. */
    basemap: string;
    places: string;
    overlays: string;
    /** A Terrarium WebP tile template. */
    terrain: string;
    /** The OSM credit, from data/sources.toml. */
    attribution: string;
    /** The credit of the basemap's land cover, from data/sources.toml. Older releases have none. */
    landcover_attribution?: string;
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

/** `value` when it is an absolute URL, such as a URL template; else undefined. */
export function configUrl(value: unknown): string | undefined {
    return typeof value === 'string' && /^[a-z]+:\/\//.test(value) ? value : undefined;
}

/** `value` as a config. A config that the planner cannot use throws. */
export function plannerConfig(value: unknown): PlannerConfig {
    if (!value || typeof value !== 'object') throw new Error('The planner config is not an object.');
    const config: any = value;
    const bounds = config.bounds;
    const urls = Object.fromEntries(URLS.map((key) => [key, configUrl(config[key])]));
    const routes = config.routes === undefined ? undefined : configUrl(config.routes);
    const layers = Object.fromEntries(Object.entries(config.layers ?? {}).map(([name, value]) => [name, configUrl(value)]));
    const invalid = [
        ...URLS.filter((key) => !urls[key]),
        ...(config.routes === undefined || routes ? [] : ['routes']),
        ...(typeof config.name === 'string' && config.name ? [] : ['name']),
        ...(typeof config.attribution === 'string' && config.attribution ? [] : ['attribution']),
        ...(config.landcover_attribution === undefined || typeof config.landcover_attribution === 'string' ? [] : ['landcover_attribution']),
        ...(typeof config.terrain_attribution === 'string' ? [] : ['terrain_attribution']),
        ...(config.layers && typeof config.layers === 'object' && Object.values(layers).every(Boolean) ? [] : ['layers']),
        ...(Array.isArray(bounds) && bounds.length === 4 && bounds.every(Number.isFinite) && bounds[0] >= -180 && bounds[2] <= 180
            && bounds[1] >= -85 && bounds[3] <= 85 && bounds[0] < bounds[2] && bounds[1] < bounds[3] ? [] : ['bounds']),
    ];
    if (invalid.length) throw new Error(`The planner config has invalid fields: ${invalid.join(', ')}.`);
    return { ...config, ...urls, routes, layers };
}
