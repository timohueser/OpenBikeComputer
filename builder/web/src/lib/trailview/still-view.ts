import type { PlannerConfig } from '../planner/config';
import { worldPixel } from '../planner/layers/mercator';
import { imagePixels } from '../planner/layers/terrain';
import { instantAt, solarPosition, solarTerms } from '../planner/solar';
import { MAX_ALTITUDE, VFOV } from './frame/camera';
import { CURVATURE, frameAt } from './frame/frame';
import { heightOf } from './stream/heights';
import { releaseTiles } from './stream/terrain';
import { TIME_ZONE, type View } from './view';

const RAD = Math.PI / 180;
/** The headings that the search tests, and how near the sun's bearing no view looks: into the sun, lakes turn white and the haze grows. */
const HEADING_STEP = 30, SUN_DEG = 60;
/** The tilts that the search tests, from the flattest to the steepest; it halves their range down to TILT_STEP. */
const FLAT = -20, STEEP = -89, TILT_STEP = 2;
/** The share of the route that a view must show. */
const SHOWN = 0.85;
/** Each point of the route stays inside this share of the half width and the half height. */
const FILL = 0.9;
/** The points of the route that the search tests: 200 resolve the shown share to 0.5 %. */
const POINTS = 200;
/** A point stands this far above the ground. The ground hides it when it comes this near the line of sight: the clearance stands for trees. */
const POINT_M = 1, TREES_M = 20;
/** The line of sight leaves out this many cells at the point: they hold the point's own ground. */
const OWN_CELLS = 2;
/** A route less than this many metres across has no view: the frame would hold it with the eye on it. */
const LEAST_M = 10;
/** The farthest that the search moves the eye from its aim, in metres. */
const FARTHEST_M = 2_000_000;
/** The grid reaches this share of the route's box beyond it on each side: the eye of a low tilt stands outside the route. */
const PAD = 0.8;
/** Terrarium tiles of the tile service: their pixels, the deepest zoom, and the most tiles that the search loads. */
const TILE = 512, DEEPEST = 12, MAX_TILES = 4;

/** Heights in metres above sea level, NaN where none is known: cell (i, j) is the Web Mercator pixel (x0 + i, y0 + j) of a world `size` pixels wide. */
export interface HeightGrid { size: number; x0: number; y0: number; width: number; height: number; heights: Float32Array }

/** A fly view without its time and weather. */
export type Camera = Pick<View, 'lat' | 'lon' | 'heading' | 'pitch' | 'mode' | 'height'>;

type Vec = [number, number, number];
interface Axes { forward: Vec; right: Vec; up: Vec }
interface Candidate { eye: Vec; distance: number; heading: number; pitch: number; axes: Axes }

/**
 * The cover view of a line of [lon, lat] points for an image of `width` × `height` pixels, with the sun
 * at bearing `sun` in degrees: for each heading every HEADING_STEP that does not look within SUN_DEG of
 * the sun, the lowest tilt that shows SHOWN of the route, with the route centred and as near as the frame
 * permits. Of these views it takes the one that draws the route longest on the screen. The search works
 * in metres about the centre of the route, with the curvature of the Earth taken off, as the engine
 * draws it. Undefined when the route is less than LEAST_M across, when no view fits, or when the eye
 * must be higher than MAX_ALTITUDE.
 */
export function chooseView(line: readonly [number, number][], grid: HeightGrid, { width, height, sun }: { width: number; height: number; sun: number }): Camera | undefined {
    if (line.length < 2) return undefined;
    const tanV = Math.tan(VFOV / 2 * RAD), tanH = tanV * width / height;
    const lons = line.map(p => p[0]), lats = line.map(p => p[1]);
    const frame = frameAt((Math.min(...lons) + Math.max(...lons)) / 2, (Math.min(...lats) + Math.max(...lats)) / 2);
    const drop = (east: number, north: number) => CURVATURE * (east * east + north * north);
    const cellOf = (lon: number, lat: number) => {
        const [x, y] = worldPixel([lon, lat], grid.size);
        return [x - grid.x0, y - grid.y0];
    };
    const groundAt = (x: number, y: number) => {
        const i = Math.floor(x), j = Math.floor(y);
        return i >= 0 && j >= 0 && i < grid.width && j < grid.height ? grid.heights[j * grid.width + i] : NaN;
    };
    const points = resample(line, POINTS).map(([lon, lat]) => {
        const [east, north] = frame.local(lon, lat), [x, y] = cellOf(lon, lat);
        return { at: [east, north, groundAt(x, y) + POINT_M - drop(east, north)] as Vec, x, y };
    });
    if (points.some(({ at }) => Number.isNaN(at[2]))) return undefined;
    const across = (axis: 0 | 1) => Math.max(...points.map(({ at }) => at[axis])) - Math.min(...points.map(({ at }) => at[axis]));
    if (Math.max(across(0), across(1)) < LEAST_M) return undefined;
    const allowed = Math.floor((1 - SHOWN) * points.length);

    /** Whether no more than `allowed` points are hidden from `eye`: a point is hidden where the ground comes within TREES_M of its line of sight. */
    const shows = (eye: Vec) => {
        const [ex, ey] = cellOf(...frame.lonLat(eye[0], eye[1]));
        let hidden = 0;
        for (const { at, x, y } of points) {
            const dx = x - ex, dy = y - ey, steps = Math.ceil(Math.max(Math.abs(dx), Math.abs(dy)));
            for (let s = 1; s < steps - OWN_CELLS; s++) {
                const t = s / steps, east = eye[0] + t * (at[0] - eye[0]), north = eye[1] + t * (at[1] - eye[1]);
                if (groundAt(ex + t * dx, ey + t * dy) - drop(east, north) > eye[2] + t * (at[2] - eye[2]) - TREES_M) {
                    if (++hidden > allowed) return false;
                    break;
                }
            }
        }
        return true;
    };
    /** Each point on the screen, from −1 to 1 across the frame; Infinity behind the eye. */
    const project = (eye: Vec, { forward, right, up }: Axes) => points.map(({ at }) => {
        const v: Vec = [at[0] - eye[0], at[1] - eye[1], at[2] - eye[2]], z = dot(v, forward);
        return z > 0 ? [dot(v, right) / z / tanH, dot(v, up) / z / tanV] : [Infinity, Infinity];
    });
    /** The nearest eye on the line of sight through `aim` that holds every point inside FILL of the frame. */
    const fit = (aim: Vec, axes: Axes) => {
        const eyeAt = (d: number): Vec => [aim[0] - axes.forward[0] * d, aim[1] - axes.forward[1] * d, aim[2] - axes.forward[2] * d];
        let near = 0, far = FARTHEST_M;
        for (let i = 0; i < 24; i++) {
            const mid = (near + far) / 2;
            if (project(eyeAt(mid), axes).every(([x, y]) => Math.abs(x) < FILL && Math.abs(y) < FILL)) far = mid;
            else near = mid;
        }
        return { eye: eyeAt(far), distance: far };
    };
    const base = points.reduce((sum, { at }) => sum + at[2], 0) / points.length;
    /** The view at a heading and a tilt: the aim moves until the route's extent on the screen is centred. */
    const view = (heading: number, pitch: number): Candidate => {
        const axes = axesOf(heading, pitch);
        let aim: Vec = [0, 0, base], fitted = fit(aim, axes);
        for (let pass = 0; pass < 3; pass++) {
            const screen = project(fitted.eye, axes), xs = screen.map(p => p[0]), ys = screen.map(p => p[1]);
            const cx = (Math.min(...xs) + Math.max(...xs)) / 2, cy = (Math.min(...ys) + Math.max(...ys)) / 2;
            aim = aim.map((a, j) => a + fitted.distance * (cx * tanH * axes.right[j] + cy * tanV * axes.up[j])) as Vec;
            fitted = fit(aim, axes);
        }
        return { ...fitted, heading, pitch, axes };
    };
    /** The length of the route on the screen in pixels. */
    const length = ({ eye, axes }: Candidate) => {
        const screen = project(eye, axes);
        let sum = 0;
        for (let i = 1; i < screen.length; i++) sum += Math.hypot((screen[i][0] - screen[i - 1][0]) * width, (screen[i][1] - screen[i - 1][1]) * height) / 2;
        return sum;
    };
    /** Heights in the search lie on the plane about the centre: the eye's altitude adds the drop back. */
    const altitude = ({ eye }: Candidate) => eye[2] + drop(eye[0], eye[1]);

    let best: { candidate: Candidate; length: number } | undefined;
    for (let heading = 0; heading < 360; heading += HEADING_STEP) {
        if (Math.abs(((heading - sun) % 360 + 540) % 360 - 180) < SUN_DEG) continue;
        // The shown share grows as the tilt steepens.
        let lowest: Candidate | undefined = view(heading, FLAT);
        if (!shows(lowest.eye)) {
            lowest = view(heading, STEEP);
            if (!shows(lowest.eye)) continue;
            for (let flat = FLAT, steep = STEEP; flat - steep > TILT_STEP;) {
                const tilt = Math.round((flat + steep) / 2), candidate = view(heading, tilt);
                if (shows(candidate.eye)) [steep, lowest] = [tilt, candidate];
                else flat = tilt;
            }
        }
        if (altitude(lowest) > MAX_ALTITUDE) continue;
        const drawn = length(lowest);
        if (!best || drawn > best.length) best = { candidate: lowest, length: drawn };
    }
    if (!best) return undefined;
    const { eye, heading, pitch } = best.candidate, [lon, lat] = frame.lonLat(eye[0], eye[1]);
    // A view's heading is from true north at the eye; the frame's north turns from it away from the centre.
    const [east, north] = frame.local(lon, lat + 1e-4), convergence = Math.atan2(east - eye[0], north - eye[1]) / RAD;
    return { lat, lon, heading: ((heading - convergence) % 360 + 360) % 360, pitch, mode: 'fly', height: altitude(best.candidate) };
}

/**
 * The cover view of a line of [lon, lat] points (`chooseView`) from the terrain tiles of the release,
 * lit by the sun at the date and the time of the view. The grid comes from the deepest zoom at which the
 * route's box, with PAD of its size beyond it on each side, needs at most MAX_TILES tiles. Undefined when
 * the route leaves the bounds of the release, or when no view fits.
 */
export async function coverView(line: readonly [number, number][], config: PlannerConfig, { width, height, date, time, weather }: { width: number; height: number } & Pick<View, 'date' | 'time' | 'weather'>): Promise<View | undefined> {
    const [west, south, east, north] = config.bounds;
    if (!line.length || line.some(([lon, lat]) => lon < west || lon > east || lat < south || lat > north)) return undefined;
    const lons = line.map(p => p[0]), lats = line.map(p => p[1]);
    const box = [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)], centre: [number, number] = [(box[0] + box[2]) / 2, (box[1] + box[3]) / 2];
    // The pad is a share of the longer side, in degrees of latitude: a long thin route leaves its eye room to the side.
    const pad = PAD * Math.max((box[2] - box[0]) * Math.cos(centre[1] * RAD), box[3] - box[1]), padLon = pad / Math.cos(centre[1] * RAD);
    const tilesAt = (z: number) => {
        const [x0, y0] = worldPixel([box[0] - padLon, box[3] + pad], 2 ** z).map(Math.floor), [x1, y1] = worldPixel([box[2] + padLon, box[1] - pad], 2 ** z).map(Math.floor);
        return { z, x0, y0, columns: x1 - x0 + 1, rows: y1 - y0 + 1 };
    };
    let tiles = tilesAt(DEEPEST);
    while (tiles.z > 0 && tiles.columns * tiles.rows > MAX_TILES) tiles = tilesAt(tiles.z - 1);
    const grid = await heightGrid(config, tiles);
    const minutes = time.split(':').map(Number).reduce((sum, part, i) => sum + part * [60, 1, 1 / 60][i], 0);
    let instant: number;
    try {
        instant = instantAt(date, minutes, TIME_ZONE);
    } catch {
        // The clock skips this time when it changes to summer time.
        instant = instantAt(date, minutes + 60, TIME_ZONE);
    }
    const sun = solarPosition(centre, solarTerms(instant)).bearing / RAD;
    const camera = chooseView(line, grid, { width, height, sun });
    return camera && { ...camera, date, time, weather, debug: 'off' };
}

/** The heights of a rectangle of tiles at zoom `z`, decoded into one grid. */
async function heightGrid(config: PlannerConfig, { z, x0, y0, columns, rows }: { z: number; x0: number; y0: number; columns: number; rows: number }): Promise<HeightGrid> {
    const width = columns * TILE, height = rows * TILE, heights = new Float32Array(width * height).fill(NaN), fetch = releaseTiles(config);
    await Promise.all(Array.from({ length: columns * rows }, async (_, k) => {
        const column = k % columns, row = Math.floor(k / columns), body = await fetch(z, x0 + column, y0 + row);
        if (!body) return;
        const { data, width: side } = await imagePixels(new Uint8Array(body));
        if (side !== TILE) throw new Error(`A terrain tile has ${side} px, not ${TILE}.`);
        for (let j = 0; j < TILE; j++) for (let i = 0; i < TILE; i++) heights[(row * TILE + j) * width + column * TILE + i] = heightOf(data, 4 * (j * TILE + i));
    }));
    return { size: TILE * 2 ** z, x0: x0 * TILE, y0: y0 * TILE, width, height, heights };
}

/** `count` points at even distances along the line. */
function resample(line: readonly [number, number][], count: number): [number, number][] {
    const along = [0];
    for (let i = 1; i < line.length; i++) along.push(along[i - 1] + Math.hypot((line[i][0] - line[i - 1][0]) * Math.cos(line[i][1] * RAD), line[i][1] - line[i - 1][1]));
    const out: [number, number][] = [];
    for (let s = 0, i = 0; s < count; s++) {
        const t = along.at(-1)! * s / (count - 1);
        while (i < line.length - 2 && along[i + 1] < t) i++;
        const f = along[i + 1] > along[i] ? (t - along[i]) / (along[i + 1] - along[i]) : 0;
        out.push([line[i][0] + f * (line[i + 1][0] - line[i][0]), line[i][1] + f * (line[i + 1][1] - line[i][1])]);
    }
    return out;
}

const dot = (a: Vec, b: Vec) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

/** The camera's axes, as `viewProjection` of frame/camera.ts makes them. */
function axesOf(heading: number, pitch: number): Axes {
    const h = heading * RAD, p = pitch * RAD;
    return {
        forward: [Math.sin(h) * Math.cos(p), Math.cos(h) * Math.cos(p), Math.sin(p)],
        right: [Math.cos(h), -Math.sin(h), 0],
        up: [-Math.sin(h) * Math.sin(p), -Math.cos(h) * Math.sin(p), Math.cos(p)],
    };
}
