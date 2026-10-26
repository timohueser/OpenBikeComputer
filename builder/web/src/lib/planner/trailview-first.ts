// The first view at a drop of the binoculars. The eye steps onto a road or path that looks "on" the
// drop, and out of a house. It looks along the route, along the road, or to the farthest view.
import { WALK_EYE } from '../trailview/frame/camera';
import { decodeFeatures, EXTENT, type Feature } from '../trailview/stream/features';
import { featureTiles } from '../trailview/stream/terrain';
import type { PlannerConfig } from './config';
import { kilometres, nearestOnLine, type Coordinate } from './geo';
import { fetchTile } from './layers/archive';
import { latitudeAt, worldPixel } from './layers/mercator';
import { terrainHeights, UNKNOWN_HEIGHT } from './layers/terrain';
import { DEM_TILE } from './map-style';

/** Metres east and north of the drop. */
export type Xy = [number, number];
/** The rings of one house: outer rings and their holes. */
type House = Xy[][];

/** What the planner knows at the drop, beside its coordinate. */
export interface Near {
    /** Metres on the ground per CSS pixel at the drop, at the zoom of the map. */
    metresPerPx: number;
    /** The plan's route in riding order; empty without a shown route. */
    route: readonly Coordinate[];
}

export interface FirstView { lon: number; lat: number; heading: number }

/** A drop this many CSS pixels from a line is on it. */
const ON_LINE_PX = 12;
/** The buffer of a feature tile (specs/trailview-features.md): within it, the tile of the drop holds every line. */
const MAX_SNAP_M = 40;
/** The eye stands this far outside a house wall. */
const WALL_GAP_M = 2;
const FEATURE_ZOOM = 13;
const ROADS = new Set(['main-road', 'road', 'path']);
const TERRAIN_ZOOM = 10;
const REACH_M = 20_000, STEP_M = 50;
/** A load that is not done by then is left out: the engine does not wait for the first view. */
const DEADLINE_MS = 1500;
/** The Web Mercator sphere. */
const EARTH_M = 6_378_137;
const RAD = Math.PI / 180;

/** Where the eye starts and where it looks, from the drop. A failed or slow load leaves the eye at the drop, looking north. */
export async function firstView(config: PlannerConfig, at: Coordinate, near: Near): Promise<FirstView> {
    const radius = Math.min(ON_LINE_PX * near.metresPerPx, MAX_SNAP_M);
    const route = routeView(near.route, at, radius);
    if (route) return route;
    const deadline = new Promise<undefined>(resolve => setTimeout(resolve, DEADLINE_MS));
    const settled = <T>(load: Promise<T>) => Promise.race([load.catch(error => void console.warn('TrailView first view:', error)), deadline]);
    const [features, height] = await Promise.all([settled(dropFeatures(config, at)), settled(terrainNear(config, at))]);
    const { eye, along } = features ? place(features, at, radius) : { eye: [0, 0] as Xy, along: undefined };
    const headings = along === undefined ? Array.from({ length: 36 }, (_, i) => i * 10) : [along, (along + 180) % 360];
    return { ...coordinateOf(at, eye), heading: height ? farthest(height, eye, headings) : along ?? 0 };
}

/** On the route: the eye on it, looking in the riding direction. */
export function routeView(route: readonly Coordinate[], at: Coordinate, radius: number): FirstView | undefined {
    if (route.length < 2) return undefined;
    const { index, at: on } = nearestOnLine(route, at);
    if (kilometres(on, at) * 1000 > radius) return undefined;
    const from = route[index], to = route.slice(index + 1).find(c => c[0] !== from[0] || c[1] !== from[1]);
    if (!to) return undefined;
    return { lon: on[0], lat: on[1], heading: bearing([(to[0] - from[0]) * Math.cos(from[1] * RAD), to[1] - from[1]]) };
}

/** The eye on the nearest road or path within `radius`, and the direction of that road; then out of any house. */
function place(features: Feature[], at: Coordinate, radius: number): { eye: Xy; along?: number } {
    const { lines, houses } = localGeometry(features, at);
    const snapped = snap([0, 0], lines, radius);
    return { eye: outside(snapped?.at ?? [0, 0], houses, WALL_GAP_M), along: snapped?.along };
}

/** The nearest point within `radius` on `lines`, and the bearing of its segment. */
export function snap(point: Xy, lines: Xy[][], radius: number): { at: Xy; along: number } | undefined {
    let found: { at: Xy; along: number } | undefined, nearest = radius;
    for (const line of lines) {
        for (let i = 1; i < line.length; i++) {
            const at = closest(point, line[i - 1], line[i]), distance = Math.hypot(at[0] - point[0], at[1] - point[1]);
            if (distance > nearest) continue;
            nearest = distance;
            found = { at, along: bearing([line[i][0] - line[i - 1][0], line[i][1] - line[i - 1][1]]) };
        }
    }
    return found;
}

/**
 * `point`, or when it lies in a house, the nearest point `gap` metres outside a wall that lies in no house.
 * A wall that two row houses share has a house on both sides, so the eye leaves by another wall.
 */
export function outside(point: Xy, houses: House[], gap: number): Xy {
    const housed = (p: Xy) => houses.some(rings => contains(rings, p));
    if (!housed(point)) return point;
    let found = point, nearest = Infinity;
    for (const rings of houses) {
        for (const ring of rings) {
            ring.forEach((b, i) => {
                const a = ring[(i || ring.length) - 1], wall = closest(point, a, b), distance = Math.hypot(wall[0] - point[0], wall[1] - point[1]);
                const length = Math.hypot(b[0] - a[0], b[1] - a[1]);
                if (distance >= nearest || !length) return;
                const normal: Xy = [(b[1] - a[1]) / length * gap, (a[0] - b[0]) / length * gap];
                for (const side of [1, -1]) {
                    const step: Xy = [wall[0] + side * normal[0], wall[1] + side * normal[1]];
                    if (housed(step)) continue;
                    found = step;
                    nearest = distance;
                    break;
                }
            });
        }
    }
    return found;
}

/** Whether the rings hold `point`: an odd number of crossings, so a point in a courtyard is outside. */
function contains(rings: Xy[][], [x, y]: Xy): boolean {
    let inside = false;
    for (const ring of rings) {
        for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
            const [xi, yi] = ring[i], [xj, yj] = ring[j];
            if ((yi > y) !== (yj > y) && x < xi + (y - yi) / (yj - yi) * (xj - xi)) inside = !inside;
        }
    }
    return inside;
}

function closest(p: Xy, a: Xy, b: Xy): Xy {
    const dx = b[0] - a[0], dy = b[1] - a[1];
    const t = Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy || 1)));
    return [a[0] + t * dx, a[1] + t * dy];
}

/** Degrees clockwise from north of a step east and north. */
const bearing = ([east, north]: Xy) => (Math.atan2(east, north) / RAD + 360) % 360;

/** What a direction sees: how far the farthest visible ground lies, and the steepest angle of its horizon. */
export interface Sight { heading: number; distance: number; horizon: number }

/** The ground that the eye sees along `heading`: a sample is visible when no nearer sample rises above its line of sight. */
export function sight(height: (point: Xy) => number, eye: Xy, heading: number): Sight {
    const top = height(eye) + WALK_EYE, east = Math.sin(heading * RAD), north = Math.cos(heading * RAD);
    let distance = 0, horizon = -Infinity;
    for (let d = STEP_M; d <= REACH_M && Number.isFinite(top); d += STEP_M) {
        const slope = (height([eye[0] + d * east, eye[1] + d * north]) - top) / d;
        if (!(slope >= horizon)) continue;
        horizon = slope;
        distance = d;
    }
    return { heading, distance, horizon };
}

/** Whether `a` is the better first view: the longer view, then the lower horizon. */
export const better = (a: Sight, b: Sight) => a.distance !== b.distance ? a.distance > b.distance : a.horizon < b.horizon;

/** The heading of the best sight of `headings`; the first heading when the eye has no height. */
export function farthest(height: (point: Xy) => number, eye: Xy, headings: number[]): number {
    return headings.map(heading => sight(height, eye, heading)).reduce((best, next) => better(next, best) ? next : best).heading;
}

/** World units of the feature tiles: tile units across the world at the feature zoom. */
const FEATURE_WORLD = 2 ** FEATURE_ZOOM * EXTENT;
const metresPerUnit = (latitude: number, world: number) => 2 * Math.PI * EARTH_M * Math.cos(latitude * RAD) / world;

/** The coordinate `eye` metres from `at`. */
function coordinateOf(at: Coordinate, [east, north]: Xy): { lon: number; lat: number } {
    const [x, y] = worldPixel(at, FEATURE_WORLD), scale = metresPerUnit(at[1], FEATURE_WORLD);
    return { lon: (x + east / scale) / FEATURE_WORLD * 360 - 180, lat: latitudeAt(y - north / scale, FEATURE_WORLD) };
}

/** The features of the feature tile under `at`; none when the release has no feature layer. */
async function dropFeatures(config: PlannerConfig, at: Coordinate): Promise<Feature[]> {
    const [x, y] = worldPixel(at, 2 ** FEATURE_ZOOM).map(Math.floor);
    const body = await featureTiles(config)?.(FEATURE_ZOOM, x, y);
    if (!body) return [];
    let bytes = new Uint8Array(body);
    // A tile service may hand over the gzip of the archive as it is.
    if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
    return decodeFeatures(bytes);
}

/** The roads and paths on the surface, and the houses near the drop, in metres from `at`. */
export function localGeometry(features: Feature[], at: Coordinate): { lines: Xy[][]; houses: House[] } {
    const [x, y] = worldPixel(at, 2 ** FEATURE_ZOOM), tx = Math.floor(x), ty = Math.floor(y);
    const scale = metresPerUnit(at[1], FEATURE_WORLD), u = (x - tx) * EXTENT, v = (y - ty) * EXTENT;
    const local = (xy: Int32Array): Xy[] => Array.from({ length: xy.length / 2 }, (_, i) => [(xy[2 * i] - u) * scale, (v - xy[2 * i + 1]) * scale]);
    // A house farther than this cannot hold the eye, which moves at most MAX_SNAP_M.
    const near = (ring: Xy[]) => ring.some(([east, north]) => Math.hypot(east, north) < 200);
    const lines: Xy[][] = [], houses: House[] = [];
    for (const feature of features) {
        if (feature.geometry === 'line' && ROADS.has(feature.kind) && !(Number(feature.tags.layer) < 0)) lines.push(...feature.parts.map(part => local(part.xy)));
        if (feature.geometry === 'area' && feature.kind === 'building') {
            const rings = feature.parts.map(part => local(part.xy));
            if (rings.some(near)) houses.push(rings);
        }
    }
    return { lines, houses };
}

/** The terrain height in metres at a point in metres from `at`, from the zoom 10 tiles within the reach; NaN where unknown. */
async function terrainNear(config: PlannerConfig, at: Coordinate): Promise<(point: Xy) => number> {
    const world = DEM_TILE * 2 ** TERRAIN_ZOOM, [x, y] = worldPixel(at, world), scale = metresPerUnit(at[1], world);
    const reach = (REACH_M + MAX_SNAP_M) / scale / DEM_TILE, tiles = new Map<string, Int16Array | null>();
    const source = terrainHeights((z, x, y, signal) => fetchTile(config.terrain, z, x, y, signal));
    const loads = [];
    for (let tx = Math.floor(x / DEM_TILE - reach); tx <= Math.floor(x / DEM_TILE + reach); tx++) {
        for (let ty = Math.floor(y / DEM_TILE - reach); ty <= Math.floor(y / DEM_TILE + reach); ty++) {
            loads.push(source.tile(TERRAIN_ZOOM, tx, ty).then(tile => void tiles.set(`${tx}/${ty}`, tile)));
        }
    }
    await Promise.all(loads);
    const pixel = (px: number, py: number) => {
        const tile = tiles.get(`${Math.floor(px / DEM_TILE)}/${Math.floor(py / DEM_TILE)}`), value = tile?.[(py % DEM_TILE) * DEM_TILE + px % DEM_TILE];
        return value === undefined || value === UNKNOWN_HEIGHT ? NaN : value;
    };
    // Bilinear between pixel centres.
    return ([east, north]) => {
        const px = x + east / scale - .5, py = y - north / scale - .5, left = Math.floor(px), top = Math.floor(py), fx = px - left, fy = py - top;
        return (pixel(left, top) * (1 - fx) + pixel(left + 1, top) * fx) * (1 - fy) + (pixel(left, top + 1) * (1 - fx) + pixel(left + 1, top + 1) * fx) * fy;
    };
}
