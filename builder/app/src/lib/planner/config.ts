import type { Archive } from './layers/data-layer';

export type Bounds = [number, number, number, number];

/**
 * The planner config: the `active` entry of the planner catalogue in specs/planner-release.md, which a
 * planner build carries as `VITE_PLANNER_CONFIG`. A local preview gives the same fields with local URLs.
 * Every URL is absolute, because the contour workers resolve no relative URL.
 */
export interface PlannerConfig {
    name: string;
    bounds: Bounds;
    /** TileJSON URLs; a local preview gives `pmtiles://` archive URLs. */
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
const absolute = (value: unknown) => typeof value === 'string' && /^[a-z]+:\/\//.test(value);

/** The config in `text`. A config that the planner cannot use throws, so the build that carries it fails. */
export function plannerConfig(text: string | undefined): PlannerConfig {
    if (!text) throw new Error('Set VITE_PLANNER_CONFIG to the planner config; see the planner README.');
    const config = JSON.parse(text);
    const bounds = config.bounds;
    const invalid = [
        ...URLS.filter((key) => !absolute(config[key])),
        ...(config.routes === undefined || absolute(config.routes) ? [] : ['routes']),
        ...(typeof config.name === 'string' && config.name ? [] : ['name']),
        ...(typeof config.terrain_attribution === 'string' ? [] : ['terrain_attribution']),
        ...(config.layers && typeof config.layers === 'object' && Object.values(config.layers).every(absolute) ? [] : ['layers']),
        ...(Array.isArray(bounds) && bounds.length === 4 && bounds.every(Number.isFinite) && bounds[0] >= -180 && bounds[2] <= 180
            && bounds[1] >= -85 && bounds[3] <= 85 && bounds[0] < bounds[2] && bounds[1] < bounds[3] ? [] : ['bounds']),
    ];
    if (invalid.length) throw new Error(`The planner config has invalid fields: ${invalid.join(', ')}.`);
    return config;
}
