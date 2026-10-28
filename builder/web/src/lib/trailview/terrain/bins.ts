// The stamp data of a terrain node from the feature tile above it: the segments, lake edges and
// junctions near the node, binned into a grid of cells, as stamp.wgsl reads them. A worker uses it.
import { EARTH_RADIUS } from '../frame/frame';
import { DEEPEST, EXTENT, unitScale, type Feature } from '../stream/features';
import { PAGE } from '../stream/heights';
import CLASS_RULES from '../../../../../../data/trailview/roads.json' with { type: 'json' };
import { boxOf, cornersOf, halfAt, partnersOf } from './corners';
import { BUILT_M, builtNear, LEFT, near, RIGHT, townOf, type Town } from './town';

/** Words of a primitive record. */
export const RECORD = 16;
/** Cells along a node edge, with one cell of apron on each side; CELL texels each. */
export const GRID = 18;
export const CELL = PAGE / (GRID - 2);
/** Words before the cell offsets, and the offsets: GRID² + 1. */
const HEAD = 2;
const OFFSETS = GRID * GRID + 1;
/** A cell entry of a lake marker: the lake's cell centre lies inside it. */
export const INSIDE = 1 << 30;
/**
 * Primitive kinds, as in stamp-core.wgsl. A stream shapes no terrain: only its water draws. A bridge holds
 * the ground under its deck, and a tunnel holds the ground over its bore. A track holds its bed as a road
 * does, and counts its sleepers where a road has its tilts. A pad holds the ground inside its edges under
 * its level, as a platform does. An area, a building or a river area, holds no ground: the mask takes its
 * inside, where nothing stands. A walk from a house to its road only paints. A corner of a junction paints
 * and holds the ground of its asphalt and its sidewalk, as corners.ts makes it.
 */
export const ROAD = 1, PATH = 2, EDGE = 3, LAKE = 4, JUNCTION = 5, RIVER = 6, STREAM = 7, BRIDGE = 8, TUNNEL = 9, RAIL = 10, PAD = 11, AREA = 12, WALK = 13, CORNER = 14;
/** Road flags: a ford, the round caps of the core and the cone at the start and the end, and of the paint where the through road runs on into another line. */
export const FORD = 1, CORE_A = 2, CORE_B = 4, CONE_A = 8, CONE_B = 16, JOIN_A = 32, JOIN_B = 64;
/** No stamp reaches further than the feature tiles' buffer, so a primitive goes to each cell within it. */
const REACH_M = 40;
/** The levels whose pages get the feature mask: they bin every road, also one that their level cannot show. */
export const NEAR = 16;
/** The lowest feature zoom. */
const LOW = 8;
const UNITS = 2 ** 30;

/**
 * The surfaces in the order of the mask's surface code (index + 1) and their half-widths without a `width` tag, from the prototype.
 * `oneway` is a one-way main road: no line in its middle. `paving` is a town's footway and a house's walk.
 * Then the ballast beds of the tracks: standard and narrow gauge, each without and with a rack.
 */
export const SURFACES = { main: 4.5, road: 3.0, gravel: 1.75, dirt: 1.5, grass: 1.25, path: 0.6, oneway: 4.5, paving: 1.25, rail: 1.9, narrow: 1.5, rack: 1.9, 'narrow-rack': 1.5 } as const;
type Surface = keyof typeof SURFACES;
const codeOf = (surface: Surface) => Object.keys(SURFACES).indexOf(surface) + 1;
/** The surface code of the first track, and of paving. */
export const RAIL_CODE = codeOf('rail'), PAVING_CODE = codeOf('paving');
/** The road surfaces that a sidewalk can run beside and a junction's corner rounds, and their codes. */
const PAVED_ROADS: Surface[] = ['main', 'road', 'oneway'];
export const PAVED_CODES = PAVED_ROADS.map(codeOf), ROAD_CODE = codeOf('road');
/** The path classes that a town paves where they have no `surface` tag. */
const TOWN_PATHS = ['footway', 'pedestrian', 'cycleway', 'steps'];
const PAVED: Record<string, number> = { motorway: 6, trunk: 5, primary: 4, secondary: 3.25, tertiary: 3, unclassified: 2.5, residential: 2.5, service: 2, living_street: 2 };
const MAIN = ['motorway', 'trunk', 'primary', 'secondary', 'motorway_link', 'trunk_link', 'primary_link'];
/** The path classes and the widest half-width of each without a `width` tag, whatever its surface: a paved footway is not a road. As PATH_HALF_M of host/obc-trailview-features/src/structures.rs. */
const PATH_HALF: Record<string, number> = { path: 1, footway: 1, cycleway: 1.5, bridleway: 1, steps: 1, pedestrian: 3, via_ferrata: 0.6 };
/** OSM bridge values of a way that lies on the ground, as GROUND of host/obc-trailview-features/src/structures.rs. */
const GROUND = ['no', 'boardwalk', 'low_water_crossing'];
/** The width of the sidewalks of the road classes that have them in a town. */
const SIDEWALK_M: Record<string, number> = {
    primary: 2, primary_link: 2, secondary: 2, secondary_link: 2, tertiary: 2, tertiary_link: 2, unclassified: 1.5, residential: 1.5,
};
/** The buildings that hold cars: a driveway leads to them. */
const CARS = ['garage', 'garages', 'carport'];
/** The deck depth of a bridge without the `deck` tag of the bake, and the height of a tunnel's bore for a road, a path and a track, as `bore` of host/obc-trailview-features/src/structures.rs. */
const DECK_M = 0.6, BORE_M = 5, BORE_PATH_M = 2.6, BORE_RAIL_M = 6.5;
/** The top of a gallery's roof at its hill, over a road, a path and a track. */
export const ROOF_M = 6.2, ROOF_PATH_M = 4, ROOF_RAIL_M = 7.5;
/** A gauge narrower than this many mm is narrow, as NARROW_MM of host/obc-trailview-features/src/rails.rs. */
const NARROW_MM = 1200;
/** Sleepers lie about this far apart: a whole number of them between two OSM vertices of a track. */
export const SLEEPER_M = 0.6;
/** The ground under a platform lies this far under its lowest top, from the levels of the structures' nodes on. */
const PLATFORM_GAP_M = 0.1, PAD_LEVEL = 15;
/** A bridge on piers, one with a roof and walls, or one that hangs from cables. */
export type BridgeStyle = 'beam' | 'covered' | 'cable';
const CABLES = ['suspension', 'simple-suspension', 'cable-stayed'];
/**
 * The profile of a road or path class, as `Profile` in stamp-core.wgsl. The core holds the road height
 * ± `tol`. The cut and the fill (rise per metre) widen until they stop binding `reach` metres past the
 * core, or for a path past its edge: its bench. A level stamps a road only where
 * `least` metres of its cut and fill fit inside the buffer. A path's bench may follow the hillside
 * across it by `tilt` metres per metre. A road in a town has a sidewalk `walk` metres wide on each side where a
 * building is near, at the height of its edge.
 */
export interface Profile { tol: number; reach: number; least: number; cut: number; fill: number; tilt: number; walk: number }
/** A river's water lies flat on its core; banks of 27° near the water stay under its ribbon's edge. */
const RIVER_BANKS: Profile = { tol: 0.02, reach: 15, least: 2, cut: 0.5, fill: 0.5, tilt: 0, walk: 0 };
/** One row per `highway` class of the road and path kinds, from the class rules that the road solver of the bake reads too, and the rivers. */
export const PROFILES: Record<string, Profile> = {
    ...Object.fromEntries(Object.entries(CLASS_RULES).map(([name, { tol, reach, least, cut, fill, tilt }]) => [name, { tol, reach, least, cut, fill, tilt, walk: SIDEWALK_M[name] ?? 0 }])),
    river: RIVER_BANKS,
};
const CLASSES = Object.keys(PROFILES);
/** Whether a way is a track, as `is_track` of host/obc-trailview-features/src/structures.rs: a `railway` of these and no `highway`. */
const TRACKS = ['rail', 'narrow_gauge', 'light_rail', 'funicular'];
const isTrack = (tags: Record<string, string>) => TRACKS.includes(tags.railway) && !tags.highway;
/** The class of a way, as `class_of` of host/obc-trailview-features/src/structures.rs: `rack` for a rack railway, else the `railway` of a track or its `highway`. */
const classOf = (tags: Record<string, string>) => (isTrack(tags) ? (tags.rack && tags.rack !== 'no' ? 'rack' : tags.railway) : tags.highway ?? '');
/** The row of a line's class; a class that the table lacks takes the row of its kind. */
const profileOf = (kind: string, name = '') => CLASSES.indexOf(kind === 'river' ? 'river' : Object.hasOwn(PROFILES, name) ? name : kind === 'path' ? 'path' : 'road');
/** The table as stamp-core.wgsl reads it: PROFILE_ROWS rows of 8 floats. */
export const PROFILE_ROWS = 32, PROFILE_FLOATS = 8;
export function profileTable(): Float32Array<ArrayBuffer> {
    const rows = Object.values(PROFILES), table = new Float32Array(PROFILE_ROWS * PROFILE_FLOATS);
    if (rows.length > PROFILE_ROWS) throw new Error(`stamp-core.wgsl holds ${PROFILE_ROWS} profiles, not ${rows.length}.`);
    rows.forEach(({ tol, reach, least, cut, fill, tilt, walk }, i) => table.set([tol, reach, least, cut, fill, tilt, walk], PROFILE_FLOATS * i));
    return table;
}
/** The half-width of a waterway without a `width` tag. */
const WATERWAYS: Record<string, number> = { river: 5, canal: 5, stream: 1 };

export interface Node { level: number; x: number; y: number }

/**
 * The feature tile that a node reads: the zoom above its own level, as a terrain tile holds four
 * nodes, or DEEPEST, which holds every kind; none above zoom 8.
 */
export function featureTile({ level, x, y }: Node): Node | undefined {
    const z = Math.min(level - 1, DEEPEST);
    return z < LOW ? undefined : { level: z, x: x >> (level - z), y: y >> (level - z) };
}

/** The prototype's road surface of a highway (`road_surface` in its bake); `town` where a building is near. */
export function surfaceOf(tags: Record<string, string>, town = false): Surface {
    const { highway = '', surface = '', tracktype = '', oneway = '', junction = '' } = tags;
    // A motorway and a roundabout are one-way unless OSM says otherwise. Traffic that takes turns, or
    // changes its direction, keeps no line in the middle either.
    const implied = ['motorway', 'motorway_link'].includes(highway) || junction === 'roundabout';
    const main = (oneway ? ['yes', 'true', '1', '-1', 'reverse', 'reversible', 'alternating'].includes(oneway) : implied) ? 'oneway' : 'main';
    if (['asphalt', 'paved', 'concrete', 'paving_stones', 'sett', 'chipseal'].includes(surface)) return MAIN.slice(0, 4).includes(highway) ? main : 'road';
    if (['gravel', 'compacted', 'fine_gravel', 'pebblestone'].includes(surface)) return 'gravel';
    if (['dirt', 'ground', 'earth', 'mud', 'unpaved', 'sand'].includes(surface)) return 'dirt';
    if (['grass', 'grass_paver'].includes(surface)) return 'grass';
    if (town && TOWN_PATHS.includes(highway)) return 'paving';
    // A path without a `surface` tag is a path's, but a cycleway is paved.
    if (Object.hasOwn(PATH_HALF, highway) && highway !== 'cycleway') return 'path';
    if (highway === 'track') return ({ grade1: 'road', grade2: 'gravel', grade3: 'dirt' } as const)[tracktype as 'grade1'] ?? 'grass';
    return MAIN.includes(highway) ? main : 'road';
}

/** The ballast bed of a track: by its gauge in mm, else by its class, and its rack. */
export function trackSurface(tags: Record<string, string>): Surface {
    const gauge = parseInt(tags.gauge ?? '', 10);
    const narrow = Number.isFinite(gauge) ? gauge < NARROW_MM : ['narrow_gauge', 'funicular'].includes(tags.railway);
    const rack = !!tags.rack && tags.rack !== 'no';
    return narrow ? (rack ? 'narrow-rack' : 'narrow') : rack ? 'rack' : 'rail';
}

/** The half-width of a road: OSM's `width` where it has one (`road_width` in the prototype). */
export function halfWidth(tags: Record<string, string>, surface: Surface): number {
    const width = parseFloat(tags.width ?? '');
    if (Number.isFinite(width)) return Math.max(width / 2, 0.4);
    const half = ['main', 'oneway', 'road'].includes(surface) && PAVED[tags.highway] ? PAVED[tags.highway] : SURFACES[surface];
    return Math.min(half, PATH_HALF[tags.highway] ?? half);
}

/** The width of a line's sidewalks where it has them. */
export const walkOf = (line: Line) => Object.values(PROFILES)[line.profile].walk;

/** The width of a road's sidewalks where a building is near: a paved road of a class that has them, but for a ford. */
export const sidewalkOf = (tags: Record<string, string>, surface: Surface) =>
    PAVED_ROADS.includes(surface) && !(tags.ford && tags.ford !== 'no') && Object.hasOwn(SIDEWALK_M, tags.highway) ? SIDEWALK_M[tags.highway] : 0;

/** Whether a line is a path, by its class: a footbridge has railings, whatever its surface. */
export const isPath = (line: Line) => Object.hasOwn(PATH_HALF, CLASSES[line.profile]);

/** Metres per world unit at world row y, on the sphere of the frame. */
export const metresPerUnit = (y: number) => 2 * Math.PI * EARTH_RADIUS / UNITS / Math.cosh(Math.PI * (1 - 2 * y / UNITS));

/**
 * A line; `anchors` marks its OSM vertices among the points of its rounded bends. `tilt` holds, per
 * segment of a road, the slopes that its core takes across it from its start and from its end, as
 * `packTilt` packs them: the through road's surface near a junction, zero elsewhere. `join` marks the
 * first and the last vertex where the line runs on into the next line of the through road at a bend.
 */
export interface Line {
    kind: number; xy: number[]; z: number[]; h: number; code: number; ford: boolean; profile: number; anchors?: boolean[]; tilt?: number[]; join?: [boolean, boolean];
    /**
     * A bridge's deck depth, a tunnel's bore height or the height of a gallery's roof at its hill, a
     * bridge's look, the side of a gallery's hill (1 left, -1 right of the line), and whether the start and the end of a bridge or a tunnel meet the ground, as `abutments` has it.
     */
    deck?: number; style?: BridgeStyle; gallery?: 1 | -1; abut?: [boolean, boolean];
    /** A track's count of sleepers from its start at each vertex, whole at its OSM vertices; whether a contact wire hangs over it. */
    sleepers?: number[]; wire?: boolean;
    /** A town road's sides with a sidewalk per segment, LEFT and RIGHT of town.ts, as townPieces decides them. */
    sidewalks?: number[];
}
/** A lake: its level, and its rings in world units; a ring after an outer ring and before the next is its hole. */
export interface Lake { level: number; rings: number[][]; outer: boolean[] }
/** A building: whether it holds cars, and the outward unit direction of the wall that its walk leaves, in world units. */
export interface Building { rings: number[][]; outer: boolean[]; cars: boolean; front?: [number, number] }
/** The shapes of a tile: lines and lakes with heights, the river areas, which lie on the drawn surface, the pads under the platforms, the buildings and the walks to them. */
export interface Shapes { lines: Line[]; lakes: Lake[]; areas: Omit<Lake, 'level'>[]; pads: Lake[]; buildings: Building[]; walks: Line[] }

/**
 * The numbers of stamp-core.wgsl that the gates of a line and the shore samples of the audit use.
 * bins.test.ts holds them equal to the shader's.
 */
export const RULES = { BUFFER_M: 40, MESH_TEXELS: 4, GATE_COS: 0.70710678, SHORE_CELLS: 1.5, PORTAL_CELLS: 2 } as const;
/** As MASK_RANGE of stamp.wgsl: the mask reads a road this far past its edge. */
const MASK_RANGE_M = 12.75;

/** As gate_texel of stamp-core.wgsl: the texel of a level at 45° N, for the gates that decide what a level stamps. */
export const gateTexel = (level: number) => 2 * Math.PI * EARTH_RADIUS * RULES.GATE_COS / 2 ** (level + 8);

export function shows({ kind, h, profile }: Line, texel: number): boolean {
    const limit = RULES.BUFFER_M - 2 * texel - 1, { reach, least } = Object.values(PROFILES)[profile];
    if (kind === STREAM || kind === WALK) return false;
    if (kind === PATH) return 2 * h >= texel && h + reach <= limit;
    return 2 * h >= texel && h + least <= limit;
}

/** The shapes of each decoded tile, made once: the stamps and the water of each of its nodes read them. */
const shapes = new WeakMap<Feature[], Shapes & { tile: string }>();

/** The roads, paths, rivers and lakes of a tile that have heights, its river areas and its buildings, in world units. */
export function features(list: Feature[], tile: Node): Shapes {
    const name = `${tile.level}/${tile.x}/${tile.y}`;
    let known = shapes.get(list);
    if (known?.tile !== name) shapes.set(list, (known = { tile: name, ...shapesOf(list, tile) }));
    return known;
}

function shapesOf(list: Feature[], tile: Node): Shapes {
    const scale = unitScale(tile.level), ox = tile.x * EXTENT, oy = tile.y * EXTENT;
    const world = (xy: Int32Array, v: number) => [(ox + xy[2 * v]) * scale, (oy + xy[2 * v + 1]) * scale];
    const lines: Line[] = [], lakes: Lake[] = [], areas: Shapes['areas'] = [], pads: Lake[] = [];
    const area = (parts: Feature['parts']) => ({ rings: parts.map(({ xy }) => Array.from({ length: xy.length / 2 }, (_, v) => world(xy, v)).flat()), outer: parts.map(part => part.outer) });
    const buildings: Building[] = list.filter(({ kind }) => kind === 'building').map(({ tags, parts }) => ({ ...area(parts), cars: CARS.includes(tags.building) }));
    const town = townOf(buildings);
    for (const { kind, tags, parts } of list) {
        if (kind === 'lake') {
            const level = parts[0]?.z?.[0];
            if (level === undefined || Number.isNaN(level)) continue;
            lakes.push({ level, ...area(parts) });
            continue;
        }
        if (kind === 'river-area') {
            areas.push(area(parts));
            continue;
        }
        if (kind === 'platform') {
            const tops = parts.flatMap(({ z }) => Array.from(z ?? []).filter(v => !Number.isNaN(v)));
            if (tops.length) pads.push({ level: Math.min(...tops) - PLATFORM_GAP_M, ...area(parts) });
            continue;
        }
        const river = kind === 'river' || kind === 'stream';
        if (!river && !['main-road', 'road', 'path', 'railway', 'tunnel', 'gallery'].includes(kind)) continue;
        // A passage through a building is a road: the building stands over it.
        const bore = kind === 'tunnel' && tags.tunnel !== 'building_passage', path = Object.hasOwn(PATH_HALF, tags.highway);
        const bridge = !river && !bore && !GROUND.includes(tags.bridge ?? 'no'), track = isTrack(tags);
        const surface = track ? trackSurface(tags) : surfaceOf(tags), width = parseFloat(tags.width ?? '');
        const h = river ? (Number.isFinite(width) ? width / 2 : WATERWAYS[tags.waterway] ?? 1) : track ? SURFACES[surface] : halfWidth(tags, surface), code = river ? 0 : Object.keys(SURFACES).indexOf(surface) + 1;
        const profile = profileOf(kind === 'tunnel' || kind === 'gallery' ? (path ? 'path' : 'road') : kind, classOf(tags));
        const type = kind === 'river' ? RIVER : kind === 'stream' ? STREAM : bridge ? BRIDGE : bore ? TUNNEL : kind === 'railway' ? RAIL : kind === 'path' || path ? PATH : ROAD;
        const deck = bridge ? parseFloat(tags.deck ?? '') || DECK_M : bore ? (track ? BORE_RAIL_M : path ? BORE_PATH_M : BORE_M) : kind === 'gallery' ? (track ? ROOF_RAIL_M : path ? ROOF_PATH_M : ROOF_M) : undefined;
        const wire = track && !bore && ['contact_line', 'yes'].includes(tags.electrified) && tags.railway !== 'funicular';
        const style: BridgeStyle | undefined = !bridge ? undefined : tags.bridge === 'covered' ? 'covered' : CABLES.includes(tags['bridge:structure']) ? 'cable' : 'beam';
        // In a town, where a building is near a piece of it: a road has a sidewalk on that side, and a footway is paved.
        const sidewalk = type === ROAD && kind !== 'gallery' ? sidewalkOf(tags, surface) : 0;
        const paving = type === PATH && kind !== 'gallery' && surfaceOf(tags, true) !== surface;
        for (const { xy, z } of parts) {
            if (!z) continue;
            // A line splits where a vertex has no height: a missing height is unknown, never 0 m. A footway splits
            // where it enters or leaves a town.
            let run: number[] = [], heights: number[] = [];
            const flush = () => {
                if (heights.length >= 2) {
                    const [shape, sides] = sidewalk || paving ? townPieces(smoothed(run, heights), town) : [smoothed(run, heights), []];
                    const n = shape.z.length, paved = (v: number) => paving && sides[v] !== 0;
                    for (let from = 0, to = 1; to < n; to++) {
                        if (to < n - 1 && paved(to) === paved(from)) continue;
                        const line: Line = {
                            kind: type, xy: shape.xy.slice(2 * from, 2 * to + 2), z: shape.z.slice(from, to + 1), anchors: shape.anchors.slice(from, to + 1),
                            h: paved(from) ? halfWidth(tags, 'paving') : h, code: paved(from) ? PAVING_CODE : code, ford: !!tags.ford && tags.ford !== 'no', profile, deck, style,
                            gallery: kind === 'gallery' ? (tags.uphill === 'right' ? -1 : 1) : undefined, wire, sidewalks: sidewalk ? sides.slice(from, to) : undefined,
                        };
                        if (type === RAIL) line.sleepers = sleepers(line);
                        lines.push(line);
                        from = to;
                    }
                }
                run = [];
                heights = [];
            };
            for (let v = 0; v < z.length; v++) {
                if (Number.isNaN(z[v])) flush();
                else {
                    run.push(...world(xy, v));
                    heights.push(z[v]);
                }
            }
            flush();
        }
    }
    junctionPlanes(lines);
    abutments(lines);
    return { lines, lakes, areas, pads, buildings, walks: walks(lines, buildings, town) };
}

/**
 * A track's sleepers from its start at each vertex: between two OSM vertices a whole number of them, about
 * SLEEPER_M apart, so the count is whole at each OSM vertex and its fraction, which the paint reads, runs
 * on unbroken from one line into the next and is the same in every tile that holds the line.
 */
function sleepers(line: Line): number[] {
    const out = [0];
    let start = 0;
    for (let v = 1; v < line.z.length; v++) {
        if (!(line.anchors?.[v] ?? true)) continue;
        const along = [0];
        for (let w = start + 1; w <= v; w++) along.push(along[w - 1 - start] + Math.hypot(line.xy[2 * w] - line.xy[2 * w - 2], line.xy[2 * w + 1] - line.xy[2 * w - 1]) * metresPerUnit(line.xy[2 * w + 1]));
        const total = along[along.length - 1], count = Math.max(1, Math.round(total / SLEEPER_M));
        for (let w = start + 1; w <= v; w++) out.push(out[start] + count * (total ? along[w - start] / total : 1));
        start = v;
    }
    return out;
}

/**
 * The ends of the bridges and the tunnels that meet the ground: an abutment, a portal. An end meets the
 * ground where it meets a road, a path or a track on it, and where a bridge and a tunnel meet: in the hillside.
 */
function abutments(lines: Line[]): void {
    const vertices = (kinds: number[]) => new Set(lines.filter(({ kind }) => kinds.includes(kind)).flatMap(({ xy }) => xy.filter((_, i) => i % 2 === 0).map((x, v) => `${x},${xy[2 * v + 1]}`)));
    const bridge = vertices([ROAD, PATH, RAIL, TUNNEL]), tunnel = vertices([ROAD, PATH, RAIL, BRIDGE]);
    for (const line of lines) {
        if (line.kind !== BRIDGE && line.kind !== TUNNEL) continue;
        const n = line.z.length, ground = line.kind === BRIDGE ? bridge : tunnel;
        line.abut = [ground.has(`${line.xy[0]},${line.xy[1]}`), ground.has(`${line.xy[2 * n - 2]},${line.xy[2 * n - 1]}`)];
    }
}

/** A town's road and footway decide their sidewalks and paving in pieces about this long. */
const PIECE_M = 8;

/**
 * A rounded line with the sides of each segment that have a building within BUILT_M: of each piece of
 * about PIECE_M, as its segment's own length places them, so each tile that holds the segment decides
 * alike. A segment is cut where the sides of its pieces change; the cuts keep its shape.
 */
function townPieces({ xy, z, anchors }: ReturnType<typeof smoothed>, town: Town): [ReturnType<typeof smoothed>, number[]] {
    const out = { xy: [xy[0], xy[1]], z: [z[0]], anchors: [anchors[0]] }, sides: number[] = [];
    for (let v = 0; v + 1 < z.length; v++) {
        const [ax, ay, bx, by] = xy.slice(2 * v, 2 * v + 4), dx = bx - ax, dy = by - ay, unit = metresPerUnit(ay), count = Math.max(1, Math.ceil(Math.hypot(dx, dy) * unit / PIECE_M));
        const pieces = Array.from({ length: count }, (_, k) => (builtNear(town, ax + dx * (k + 0.5) / count, ay + dy * (k + 0.5) / count, BUILT_M / unit) ? LEFT | RIGHT : 0));
        for (let k = 1; k <= count; k++) {
            if (k < count && pieces[k] === pieces[k - 1]) continue;
            sides.push(pieces[k - 1]);
            out.xy.push(k === count ? bx : Math.round(ax + dx * k / count), k === count ? by : Math.round(ay + dy * k / count));
            out.z.push(z[v] + (z[v + 1] - z[v]) * k / count);
            out.anchors.push(k === count && anchors[v + 1]);
        }
    }
    return [out, sides];
}

/** A walk's ends: this far inside the wall, so no slit shows; widths; the gap to the road under which it is a forecourt, the least and the most of it; the shortest wall it leaves. */
const INSIDE_M = 0.5, WALK_WIDTH_M = 1.2, DRIVEWAY_M = 3, FORECOURT_M = 6, FRONTAGE_M = [3, 15], WALL_M = 2;
/** A road or a path this near a building reaches it; a wall faces a point within 60° of its normal. */
const REACHED_M = 2, FACING_COS = 0.5;

/**
 * The walks of paving to the houses: a building whose nearest road on the ground comes within BUILT_M,
 * and no road or path within REACHED_M, gets one from the middle of its nearest wall that faces that road
 * to the road's outer edge. A garage gets a driveway, and a house nearer than FORECOURT_M a forecourt as
 * wide as that wall. Only the house and the lines near it decide, so each tile that holds them all draws it alike.
 */
function walks(lines: Line[], buildings: Building[], town: Town): Line[] {
    // Per building, the least gap to the edge of a road and that road; -Infinity where a road or a path reaches it.
    const nearest = buildings.map(() => ({ gap: BUILT_M, road: undefined as Line | undefined }));
    for (const line of lines) {
        if (line.kind !== ROAD && line.kind !== PATH) continue;
        const road = line.kind === ROAD && !line.gallery;
        for (let v = 0; v + 1 < line.z.length; v++) {
            const ax = line.xy[2 * v], ay = line.xy[2 * v + 1], bx = line.xy[2 * v + 2], by = line.xy[2 * v + 3], unit = metresPerUnit(ay);
            near(town, ax, ay, bx, by, (line.h + (road ? BUILT_M : REACHED_M)) / unit, b => {
                // The gap of the boxes bounds the ring's: a ring that this segment can neither reach nor come nearer to stays.
                const box = town.boxes[b], it = nearest[b];
                const gx = Math.max(0, box[0] - Math.max(ax, bx), Math.min(ax, bx) - box[2]), gy = Math.max(0, box[1] - Math.max(ay, by), Math.min(ay, by) - box[3]);
                const apart = Math.sqrt(gx * gx + gy * gy) * unit - line.h;
                if (apart >= REACHED_M && (!road || apart >= it.gap)) return;
                const gap = ringDistance(town.rings[b], ax, ay, bx, by) * unit - line.h;
                if (gap < REACHED_M) it.gap = -Infinity;
                else if (road && gap < it.gap) [it.gap, it.road] = [gap, line];
            });
        }
    }
    return buildings.flatMap((building, b) => (nearest[b].gap >= REACHED_M && nearest[b].road ? walkTo(building, nearest[b].road) : []));
}

/** A building's walk to a road, and the wall that it leaves as the building's front; none where no wall faces the road. */
function walkTo(building: Building, road: Line): Line[] {
    const ring = building.rings[0], n = ring.length / 2, unit = metresPerUnit(ring[1]);
    // The ring's outside lies right of each edge (y south) where its area is positive. About its first vertex: products of world units lose float64 bits.
    let twice = 0;
    for (let i = 0, j = n - 1; i < n; j = i++) twice += (ring[2 * j] - ring[0]) * (ring[2 * i + 1] - ring[1]) - (ring[2 * i] - ring[0]) * (ring[2 * j + 1] - ring[1]);
    let best: { mx: number; my: number; nx: number; ny: number; d: number; cos: number; wall: number; v: number } | undefined;
    for (let i = 0; i < n; i++) {
        const k = (i + 1) % n, length = Math.hypot(ring[2 * k] - ring[2 * i], ring[2 * k + 1] - ring[2 * i + 1]);
        if (length * unit < WALL_M) continue;
        const mx = (ring[2 * i] + ring[2 * k]) / 2, my = (ring[2 * i + 1] + ring[2 * k + 1]) / 2;
        const nx = Math.sign(twice) * (ring[2 * k + 1] - ring[2 * i + 1]) / length, ny = Math.sign(twice) * (ring[2 * i] - ring[2 * k]) / length;
        const { x, y, v } = closest(road, mx, my), d = Math.hypot(x - mx, y - my), cos = ((x - mx) * nx + (y - my) * ny) / (d || 1);
        if (cos >= FACING_COS && (!best || d < best.d)) best = { mx, my, nx, ny, d, cos, wall: length * unit, v };
    }
    if (!best) return [];
    const { mx, my, nx, ny, d, cos, wall, v } = best, [ax, ay, bx, by] = road.xy.slice(2 * v, 2 * v + 4);
    const side = (bx - ax) * (my - ay) - (by - ay) * (mx - ax) > 0 ? RIGHT : LEFT;
    // Metres along the walk from the wall to the road's edge, past its sidewalk on this side.
    const reach = (d * unit - road.h - ((road.sidewalks?.[v] ?? 0) & side ? walkOf(road) : 0)) / cos;
    if (reach <= 0) return [];
    const width = reach < FORECOURT_M ? Math.min(Math.max(wall, FRONTAGE_M[0]), FRONTAGE_M[1]) : building.cars ? DRIVEWAY_M : WALK_WIDTH_M;
    building.front = [nx, ny];
    const at = (metres: number) => [Math.round(mx + nx * metres / unit), Math.round(my + ny * metres / unit)];
    return [{ kind: WALK, xy: [...at(-INSIDE_M), ...at(reach)], z: [0, 0], h: width / 2, code: PAVING_CODE, ford: false, profile: 0, anchors: [true, true] }];
}

/** The point of a line nearest to x, y, and its segment. */
function closest(line: Line, x: number, y: number): { x: number; y: number; v: number } {
    let best = { d: Infinity, x, y, v: 0 };
    for (let v = 0; v + 1 < line.z.length; v++) {
        const ax = line.xy[2 * v], ay = line.xy[2 * v + 1], dx = line.xy[2 * v + 2] - ax, dy = line.xy[2 * v + 3] - ay;
        const t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1))), px = ax + t * dx, py = ay + t * dy, d = (px - x) ** 2 + (py - y) ** 2;
        if (d < best.d) best = { d, x: px, y: py, v };
    }
    return best;
}

/** The distance between a ring and a segment that does not cross it, in their units. */
function ringDistance(ring: number[], ax: number, ay: number, bx: number, by: number): number {
    let best = Infinity;
    for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
        best = Math.min(best, segmentDistance(ring[i], ring[i + 1], ax, ay, bx, by), segmentDistance(ax, ay, ring[j], ring[j + 1], ring[i], ring[i + 1]),
            segmentDistance(bx, by, ring[j], ring[j + 1], ring[i], ring[i + 1]));
    }
    return best;
}

/** The lines whose shared vertices are junctions of the road cores: roads, tracks, bridges and tunnels. */
export const cores = (lines: Line[]) => lines.filter(({ kind }) => kind === ROAD || kind === RAIL || kind === BRIDGE || kind === TUNNEL);

/**
 * As APPROACH_M of host/obc-trailview-features/src/roads/mod.rs: the solver puts the first metres of a side road on the
 * through road's surface. Its cross-slope holds that far, or to the through road's edge, and fades out over WARP_M.
 */
const APPROACH_M = 4;
export const WARP_M = 6;

/**
 * The slope across a segment from one of its ends, as `end_tilt` of stamp-core.wgsl reads it: the rise
 * per metre to the segment's left in steps of 1e-4 (signed, low 16 bits) and in cm how far from the end
 * it reaches (high 16 bits). It holds up to WARP_M before that and is zero past it, so a tile without the
 * junction stamps the rest of the segment as the tile with it does.
 */
const packTilt = (slope: number, reach: number) => ((Math.max(-32768, Math.min(32767, Math.round(slope * 1e4))) & 0xffff) | (Math.min(65535, Math.round(reach * 100)) << 16)) >>> 0;
const tiltAt = (word: number, metres: number) => ((word << 16) >> 16) * 1e-4 * Math.min(1, Math.max(0, ((word >>> 16) * 0.01 - metres) / WARP_M));

/**
 * The plane of the through road at each junction of roads, on the side roads. At a junction the
 * through road is the first class of roads.json there, and of two ways of that class, the pair that
 * runs most nearly straight on, as in the road solver of the bake; the solver puts each side road's
 * first metres on the surface of the through direction that it leans along most. Its core takes the
 * slope of that surface across it. Only the vertices and heights at a junction decide, so every tile
 * that holds a junction gets the same planes.
 */
function junctionPlanes(lines: Line[]): void {
    const at = new Map<string, { line: Line; v: number }[]>();
    for (const line of lines) {
        if (line.kind !== ROAD) continue;
        line.tilt = new Array(2 * (line.z.length - 1)).fill(0);
        line.z.forEach((_, v) => {
            if (!(line.anchors?.[v] ?? true)) return;
            const name = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`;
            at.set(name, [...(at.get(name) ?? []), { line, v }]);
        });
    }
    const next = (line: Line, v: number, step: number) => {
        let w = v + step;
        while (w > 0 && w < line.z.length - 1 && !(line.anchors?.[w] ?? true)) w += step;
        return w >= 0 && w < line.z.length ? w : -1;
    };
    const metres = (line: Line, a: number, b: number) => Math.hypot(line.xy[2 * b] - line.xy[2 * a], line.xy[2 * b + 1] - line.xy[2 * a + 1]) * metresPerUnit(line.xy[2 * a + 1]);
    const dot = (p: number[], q: number[]) => p[0] * q[0] + p[1] * q[1];
    for (const meeting of at.values()) {
        if (meeting.length < 2) continue;
        const ways = meeting.flatMap(({ line, v }) => [-1, 1].map(step => ({ line, v, step, w: next(line, v, step) })).filter(({ w }) => w >= 0)).map(way => {
            const { line, v, w } = way, length = Math.hypot(line.xy[2 * w] - line.xy[2 * v], line.xy[2 * w + 1] - line.xy[2 * v + 1]) || 1;
            return { ...way, dir: [(line.xy[2 * w] - line.xy[2 * v]) / length, (line.xy[2 * w + 1] - line.xy[2 * v + 1]) / length], grade: (line.z[w] - line.z[v]) / metres(line, v, w) };
        });
        if (ways.length < 2) continue;
        const best = Math.min(...ways.map(({ line }) => line.profile)), top = ways.filter(({ line }) => line.profile === best);
        let through = [top[0]];
        if (top.length >= 2) {
            let least = Infinity;
            for (let i = 0; i < top.length; i++) for (let k = i + 1; k < top.length; k++) {
                if (dot(top[i].dir, top[k].dir) < least) [least, through] = [dot(top[i].dir, top[k].dir), [top[i], top[k]]];
            }
        }
        if (through.length === 2 && through.every(({ line, v }) => v === 0 || v === line.z.length - 1)) {
            // Where OSM splits the through road at a bend, the round cap of one end paints the outer wedge.
            const [cap] = [...through].sort((p, q) => p.dir[0] - q.dir[0] || p.dir[1] - q.dir[1]);
            (cap.line.join ??= [false, false])[cap.v === 0 ? 0 : 1] = true;
        }
        const reach = Math.max(APPROACH_M, ...through.map(({ line }) => line.h)) + WARP_M;
        for (const side of ways) {
            if (through.includes(side)) continue;
            const lean = through.reduce((most, way) => (dot(way.dir, side.dir) > dot(most.dir, side.dir) ? way : most));
            const plane = [lean.grade * lean.dir[0], lean.grade * lean.dir[1]], { line, step } = side, tilt = line.tilt!;
            // Each segment within the reach takes the plane from its end nearer the junction.
            for (let v = side.v, along = 0; along < reach && v + step >= 0 && v + step < line.z.length; v += step) {
                const s = Math.min(v, v + step), end = 2 * s + (step === 1 ? 0 : 1);
                const dx = line.xy[2 * s + 2] - line.xy[2 * s], dy = line.xy[2 * s + 3] - line.xy[2 * s + 1], length = Math.hypot(dx, dy) || 1;
                const word = packTilt((plane[1] * dx - plane[0] * dy) / length, reach - along);
                if (Math.abs((word << 16) >> 16) > Math.abs((tilt[end] << 16) >> 16)) tilt[end] = word;
                along += metres(line, v, v + step);
            }
        }
    }
}

/** The slope across segment v of a road, `sa` metres from its start and `sb` from its end, as `along` in stamp-core.wgsl reads it. */
export function crossSlope(line: Line, v: number, sa: number, sb: number): number {
    return line.tilt ? tiltAt(line.tilt[2 * v], sa) + tiltAt(line.tilt[2 * v + 1], sb) : 0;
}

/** The most that a chord of a rounded bend leaves the curve, in world units (about 10 cm), and the most chords per segment. */
const SAGITTA = 4, CHORDS = 16;

/**
 * A line through its vertices with its bends rounded: each segment between two other segments follows
 * the centripetal Catmull-Rom curve through the four vertices about it, in chords of whole world
 * units, with the height linear along it. The first and last segments stay straight, also of a
 * closed way: the feature tiles cut a ring into open pieces, so every line is open. They cut a line
 * one vertex past the segments that meet a tile's buffer, so every segment that a tile's nodes stamp
 * gets the same curve in each tile that holds it; and every vertex stays, so junctions stay where they are.
 */
export function smoothed(xy: number[], z: number[]): { xy: number[]; z: number[]; anchors: boolean[] } {
    const n = z.length, segments = n - 1;
    if (n < 3) return { xy, z, anchors: z.map(() => true) };
    const point = (i: number): [number, number] => [xy[2 * i], xy[2 * i + 1]];
    const outXy = [xy[0], xy[1]], outZ = [z[0]], anchors = [true];
    for (let i = 0; i < segments; i++) {
        const [b, c] = [point(i), point(i + 1)];
        if (i > 0 && i < segments - 1) {
            const [a, d] = [point(i - 1), point(i + 2)], curve = catmullRom(a, b, c, d), middle = curve(0.5);
            const sagitta = Math.hypot(middle[0] - (b[0] + c[0]) / 2, middle[1] - (b[1] + c[1]) / 2);
            const chords = Math.min(CHORDS, Math.max(1, Math.ceil(Math.sqrt(sagitta / SAGITTA))));
            for (let k = 1; k < chords; k++) {
                const [x, y] = curve(k / chords).map(Math.round);
                if (x === outXy[outXy.length - 2] && y === outXy[outXy.length - 1]) continue;
                outXy.push(x, y);
                outZ.push(z[i] + (z[i + 1] - z[i]) * k / chords);
                anchors.push(false);
            }
        }
        outXy.push(xy[2 * (i + 1)], xy[2 * (i + 1) + 1]);
        outZ.push(z[i + 1]);
        anchors.push(true);
    }
    return { xy: outXy, z: outZ, anchors };
}

/** The centripetal Catmull-Rom curve from b to c (Barry and Goldman), as a function of 0..1. */
function catmullRom(a: number[], b: number[], c: number[], d: number[]): (u: number) => number[] {
    const knot = (p: number[], q: number[]) => Math.max(Math.sqrt(Math.hypot(q[0] - p[0], q[1] - p[1])), 1e-6);
    const t1 = knot(a, b), t2 = t1 + knot(b, c), t3 = t2 + knot(c, d);
    const lerp = (p: number[], q: number[], from: number, to: number, t: number) => p.map((v, i) => v + (q[i] - v) * (t - from) / (to - from));
    return u => {
        const t = t1 + (t2 - t1) * u;
        const a1 = lerp(a, b, 0, t1, t), a2 = lerp(b, c, t1, t2, t), a3 = lerp(c, d, t2, t3, t);
        return lerp(lerp(a1, a2, 0, t2, t), lerp(a2, a3, t1, t3, t), t1, t2, t);
    };
}

/** Junctions: vertices that two road lines meet at, or that one line passes twice (a ring's start), with the widest half-width there. */
export function junctions(lines: Line[]): Map<string, { x: number; y: number; h: number }> {
    const seen = new Map<string, { x: number; y: number; h: number; count: number }>();
    for (const line of lines) {
        for (let v = 0; v < line.z.length; v++) {
            const x = line.xy[2 * v], y = line.xy[2 * v + 1], key = `${x},${y}`;
            const entry = seen.get(key) ?? { x, y, h: 0, count: 0 };
            entry.count++;
            entry.h = Math.max(entry.h, line.h);
            seen.set(key, entry);
        }
    }
    return new Map([...seen].filter(([, { count }]) => count > 1));
}

/**
 * The unit mitre normal at vertex v of a line: along the line at an end, the bisector of the two segments
 * inside. At an OSM vertex the segments run to the OSM vertices about it, not to the points of a rounded
 * bend: a tile rounds only the segments with a vertex on both sides, so this is the same in every tile.
 */
export function mitre(line: Line, v: number): [number, number] {
    const n = line.z.length, anchored = line.anchors?.[v], dir = (a: number, b: number) => {
        const dx = line.xy[2 * b] - line.xy[2 * a], dy = line.xy[2 * b + 1] - line.xy[2 * a + 1], l = Math.hypot(dx, dy) || 1;
        return [dx / l, dy / l];
    };
    const back = (from: number) => {
        let w = from - 1;
        while (anchored && w > 0 && !line.anchors![w]) w--;
        return w;
    };
    const ahead = (from: number) => {
        let w = from + 1;
        while (anchored && w < n - 1 && !line.anchors![w]) w++;
        return w;
    };
    const before = v > 0 ? dir(back(v), v) : undefined;
    const after = v < n - 1 ? dir(v, ahead(v)) : undefined;
    const [x, y] = before && after ? [before[0] + after[0], before[1] + after[1]] : (before ?? after)!;
    const l = Math.hypot(x, y);
    return l > 1e-9 ? [x / l, y / l] : (after ?? before)! as [number, number];
}

/** Whether a point lies inside rings of world coordinates, even-odd. */
export function insideRings(rings: number[][], x: number, y: number): boolean {
    let inside = false;
    for (const ring of rings) {
        for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
            const [ax, ay, bx, by] = [ring[j], ring[j + 1], ring[i], ring[i + 1]];
            if ((ay > y) !== (by > y) && x < ax + (y - ay) * (bx - ax) / (by - ay)) inside = !inside;
        }
    }
    return inside;
}

/** The distance from a point to the nearest edge of rings, in the units of the points. */
export function ringsDistance(rings: number[][], x: number, y: number): number {
    let best = Infinity;
    for (const ring of rings) {
        for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
            const ax = ring[j], ay = ring[j + 1], dx = ring[i] - ax, dy = ring[i + 1] - ay;
            const t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
            best = Math.min(best, Math.hypot(x - ax - t * dx, y - ay - t * dy));
        }
    }
    return best;
}

/** What a node stamps: its packed data for stamp.wgsl, and what it holds. */
export interface NodeStamps {
    /** The words of stamp-core.wgsl: counts, cell offsets, cell entries, records. Null when nothing is near. */
    data: Uint32Array<ArrayBuffer> | null;
    /** A road, a path or a lake is near: the node may stamp heights. */
    heights: boolean;
}

/**
 * The stamp data of a node from the features of its feature tile. Every primitive goes to each cell
 * that it can act on, so the shader sees every primitive that can reach a texel. A cell keeps the
 * order of stamp-core.wgsl: a lake's edges before its marker, a line's segments together.
 */
export function nodeStamps(list: Feature[], tile: Node, node: Node): NodeStamps {
    const { lines, lakes, pads, areas, buildings, walks } = features(list, tile);
    const shift = 22 - node.level, cell = CELL * 2 ** shift;
    const x0 = node.x * PAGE * 2 ** shift - cell, y0 = node.y * PAGE * 2 ** shift - cell, span = GRID * cell;
    // Units per metre grow toward the pole: the node's poleward edge bounds them.
    const units = 1.02 / metresPerUnit(y0 < UNITS / 2 ? y0 - span : y0 + 2 * span);
    const records: number[][] = [];
    const cells: number[][][] = Array.from({ length: 3 }, () => Array.from({ length: GRID * GRID }, () => []));
    /** The cells within `metres` of a box. */
    const within = (box: number[], metres: number) => {
        const reach = metres * units, out: number[] = [];
        const i0 = Math.max(0, Math.floor((box[0] - reach - x0) / cell)), i1 = Math.min(GRID - 1, Math.floor((box[2] + reach - x0) / cell));
        const j0 = Math.max(0, Math.floor((box[1] - reach - y0) / cell)), j1 = Math.min(GRID - 1, Math.floor((box[3] + reach - y0) / cell));
        for (let j = j0; j <= j1; j++) for (let i = i0; i <= i1; i++) out.push(j * GRID + i);
        return out;
    };
    const record = (words: number[]) => records.push(words) - 1;
    const floats = new Float32Array(1), ints = new Uint32Array(floats.buffer);
    const f = (value: number) => ((floats[0] = value), ints[0]);

    const joins = junctions(cores(lines));
    for (const { x, y, h } of joins.values()) {
        const touched = within([x, y, x, y], REACH_M);
        if (!touched.length) continue;
        const at = record([x, y, 0, 0, JUNCTION, 0, f(h)]);
        for (const c of touched) cells[0][c].push(at);
    }
    for (const corner of cornersOf(lines)) {
        const { o, a, b, z, grad, walk, code, profile, h } = corner, touched = within(boxOf(corner), 1);
        if (!touched.length) continue;
        // The sidewalks' widths in decimetres share a word; the slopes fill the words about the second end.
        const at = record([...o, ...a, CORNER, code | (profile << 16), f(z[0]), f(z[1]), Math.round(10 * walk[0]) | (Math.round(10 * walk[1]) << 8), f(grad[0][0]), ...b, f(h), f(grad[0][1]), f(grad[1][0]), f(grad[1][1])]);
        for (const c of touched) cells[2][c].push(at);
    }
    let heights = false;
    const partners = partnersOf(lines);
    // A record holds a line's index in 16 bits.
    if (lines.length + walks.length > 0x10000) throw new Error(`nodeStamps: ${lines.length + walks.length} lines pass the 16-bit index`);
    [...lines, ...walks].forEach((line, id) => {
        const stamps = shows(line, gateTexel(node.level));
        if (line.kind === STREAM || (!stamps && node.level < NEAR)) return;
        // A path acts no farther than its bench, and on the near levels than the mask reads it; a walk only there.
        const reach = line.kind === PATH || line.kind === WALK ? line.h + (node.level >= NEAR ? MASK_RANGE_M : Object.values(PROFILES)[line.profile].reach) : REACH_M;
        const n = line.z.length, end = (v: number) => v === 0 || v === n - 1;
        // A bridge's or a tunnel's metres from its start at each vertex: it rests on its abutments, or runs on the ground into its portal, a few mesh cells from an end that meets the ground.
        const along = [0];
        for (let v = 1; v < n; v++) along.push(along[v - 1] + Math.hypot(line.xy[2 * v] - line.xy[2 * v - 2], line.xy[2 * v + 1] - line.xy[2 * v - 1]) * metresPerUnit(line.xy[2 * v + 1]));
        for (let v = 0; v + 1 < n; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4);
            const touched = within([Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)], reach);
            if (!touched.length) continue;
            const [mx, my] = mitre(line, v), [nx, ny] = mitre(line, v + 1);
            const dead = (w: number) => end(w) && !joins.has(`${line.xy[2 * w]},${line.xy[2 * w + 1]}`);
            const join = (w: number) => !!line.join?.[w === 0 ? 0 : 1] && end(w);
            const flags = line.kind === BRIDGE || line.kind === TUNNEL || line.kind === WALK ? 0 : (line.ford ? FORD : 0) | (dead(v) ? CORE_A : 0) | (dead(v + 1) ? CORE_B : 0) | (end(v) ? CONE_A : 0) | (end(v + 1) ? CONE_B : 0)
                | (join(v) ? JOIN_A : 0) | (join(v + 1) ? JOIN_B : 0);
            // The deck, the bore or the roof in decimetres; the top bit marks a gallery whose hill is on its right.
            const deck = Math.min(line.gallery ? 127 : 255, Math.round(10 * (line.deck ?? 0))) | (line.gallery === -1 ? 128 : 0);
            // A bridge's or a tunnel's metres from its ends where they meet the ground, a gallery's from its ends, a track's sleepers; else a road's tilts.
            const ends = line.abut ? [line.abut[0] ? along[v] : 1e30, line.abut[1] ? along[n - 1] - along[v + 1] : 1e30].map(f)
                : line.gallery ? [along[v], along[n - 1] - along[v + 1]].map(f) : line.sleepers ? [line.sleepers[v], line.sleepers[v + 1]].map(f)
                    : [line.tilt?.[2 * v] ?? 0, line.tilt?.[2 * v + 1] ?? 0];
            // The surface code in the low 4 bits, the sides with a sidewalk above it. Beside the line's index, in
            // steps of 2 cm, how far its edge lies out at the start and the end, where it tapers to a partner's.
            const taper = (w: number) => Math.max(-127, Math.min(127, Math.round((halfAt(line, w, partners) - line.h) / 0.02))) & 0xff;
            const at = record([ax, ay, bx, by, line.kind, line.code | ((line.sidewalks?.[v] ?? 0) << 4) | (flags << 8) | (line.profile << 16) | (deck << 24), f(line.z[v]), f(line.z[v + 1]), f(line.h), f(mx), f(my), f(nx), f(ny),
                id | (taper(v) << 16) | (taper(v + 1) << 24), ...ends]);
            for (const c of touched) cells[2][c].push(at);
            heights ||= stamps;
        }
    });
    // Per lake, pad and area near the node, its edges in each cell that they reach.
    const outlines = [...lakes.map(lake => ({ lake, kind: LAKE })), ...(node.level >= PAD_LEVEL ? pads.map(lake => ({ lake, kind: PAD })) : []),
        ...(node.level >= NEAR ? [...areas, ...buildings].map(area => ({ lake: { level: 0, ...area }, kind: AREA })) : [])];
    const all = Array.from({ length: GRID * GRID }, (_, c) => c);
    const near = outlines.flatMap(({ lake, kind }) => {
        const xs = lake.rings.flatMap(ring => ring.filter((_, i) => i % 2 === 0)), ys = lake.rings.flatMap(ring => ring.filter((_, i) => i % 2 === 1));
        // An area acts only inside it: its edges go to the cells that they cross, so a village's cells stay short.
        const metres = kind === AREA ? 0 : REACH_M, reach = metres * units;
        if (Math.max(...xs) < x0 - reach || Math.min(...xs) > x0 + span + reach || Math.max(...ys) < y0 - reach || Math.min(...ys) > y0 + span + reach) return [];
        const edges = new Map<number, number[]>();
        for (const ring of lake.rings) for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
            const [ax, ay, bx, by] = [ring[j], ring[j + 1], ring[i], ring[i + 1]];
            const touched = within([Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)], metres);
            if (!touched.length) continue;
            const at = record([ax, ay, bx, by, EDGE]);
            for (const c of touched) edges.set(c, [...(edges.get(c) ?? []), at]);
        }
        const cells = kind === AREA ? within([Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)], 0) : all;
        return [{ lake, kind, edges, cells }];
    });
    // The origin of the lake crossings of a cell: its centre, held inside the node, as stamp-core.wgsl
    // has it, and moved a few units off every lake edge of the cell, so that no rule of a point on a
    // shore line decides inside or outside.
    const nudges = new Uint32Array(GRID * GRID);
    const origin = (c: number): [number, number] => {
        const held = (v: number, base: number) => Math.min(Math.max(v, base + cell), base + cell + PAGE * 2 ** shift - 1);
        const centre = [held(x0 + (c % GRID + 0.5) * cell, x0), held(y0 + (Math.floor(c / GRID) + 0.5) * cell, y0)];
        const edges = near.flatMap(({ edges }) => edges.get(c) ?? []).map(r => records[r]);
        for (const [dx, dy] of NUDGES) {
            const [x, y] = [centre[0] + dx, centre[1] + dy];
            if (edges.every(([ax, ay, bx, by]) => segmentDistance(x, y, ax, ay, bx, by) >= 1)) {
                nudges[c] = (dx + 8) | ((dy + 8) << 4);
                return [x, y];
            }
        }
        throw new Error(`No origin off the shores in cell ${c} of node ${node.level}/${node.x}/${node.y}`);
    };
    const origins = Array.from({ length: GRID * GRID }, (_, c): [number, number] => (near.length ? origin(c) : [0, 0]));
    for (const { lake, kind, edges, cells: reached } of near) {
        // A cell lists the lake after its edges, where an edge is near or where its origin lies inside it.
        const marker = record([0, 0, 0, 0, kind, 0, f(lake.level)]);
        for (const c of reached) {
            const inside = insideRings(lake.rings, ...origins[c]);
            if (!inside && !edges.has(c)) continue;
            cells[1][c].push(...(edges.get(c) ?? []), marker | (inside ? INSIDE : 0));
        }
        heights ||= kind !== AREA;
    }
    if (!records.length || (!heights && node.level < NEAR)) return { data: null, heights: false };
    const entries = cells[0].map((_, c) => [...cells[0][c], ...cells[1][c], ...cells[2][c]]);
    const count = entries.reduce((sum, list) => sum + list.length, 0);
    const data = new Uint32Array(HEAD + OFFSETS + count + RECORD * records.length);
    data[0] = records.length;
    data[1] = count;
    let at = HEAD + OFFSETS;
    entries.forEach((list, c) => {
        data[HEAD + c] = (at - HEAD - OFFSETS) | (nudges[c] << 24);
        data.set(list, at);
        at += list.length;
    });
    data[HEAD + GRID * GRID] = count;
    records.forEach((words, r) => data.set(words.map(w => w >>> 0), HEAD + OFFSETS + count + RECORD * r));
    return { data, heights };
}

/** The moves of a cell's origin that the worker tries, in world units, as a signed nibble each. */
const NUDGES = [[0, 0], [1, 0], [0, 1], [1, 1], [2, 1], [1, 2], [-1, 2], [3, 1], [-2, 3], [3, -2], [4, 3], [-3, 4]];

function segmentDistance(x: number, y: number, ax: number, ay: number, bx: number, by: number): number {
    const dx = bx - ax, dy = by - ay, t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
    return Math.sqrt((x - ax - t * dx) ** 2 + (y - ay - t * dy) ** 2);
}
