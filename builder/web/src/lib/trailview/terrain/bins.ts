// The stamp data of a terrain node from the feature tile above it: the segments, lake edges and
// junctions near the node, binned into a grid of cells, as stamp.wgsl reads them. A worker uses it.
import { EARTH_RADIUS } from '../frame/frame';
import { DEEPEST, EXTENT, SIDE_BACK, SIDE_ON, unitScale, type Feature, type FeaturePart } from '../stream/features';
import { PAGE } from '../stream/heights';
import { addPlan, curveOf, PATH_SAG, planPart, ROAD_SAG, type Curve, type Plan } from '../stream/curve';
import CLASS_RULES from '../../../../../../data/trailview/roads.json' with { type: 'json' };
import { boxOf, cornersOf, halfAt, partnersOf } from './corners';
import { along, BUILT_M, builtNear, gridOf, LEFT, near, RIGHT, townOf, type Grid, type Town } from './town';

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
 * inside, where nothing stands. The sea holds the ground inside it under its bed, as a lake does, and
 * leaves the land as it is. A corner of a junction paints and holds the ground of its asphalt and its sidewalk, as corners.ts makes it.
 */
export const ROAD = 1, PATH = 2, EDGE = 3, LAKE = 4, JUNCTION = 5, RIVER = 6, STREAM = 7, BRIDGE = 8, TUNNEL = 9, RAIL = 10, PAD = 11, AREA = 12, SEA = 13, CORNER = 14;
/** Road flags: a ford, the round caps of the core and the cone at the start and the end, and of the paint where the through road runs on into another line, and lines on the road. */
export const FORD = 1, CORE_A = 2, CORE_B = 4, CONE_A = 8, CONE_B = 16, JOIN_A = 32, JOIN_B = 64, MARKED = 128;
/** No stamp reaches further than the feature tiles' buffer, so a primitive goes to each cell within it. */
const REACH_M = 40;
/** The levels whose pages get the feature mask: they bin every road, also one that their level cannot show. */
export const NEAR = 16;
/** The lowest feature zoom. */
const LOW = 8;
const UNITS = 2 ** 30;

/**
 * The surfaces in the order of the mask's surface code (index + 1) and their half-widths without a `width` tag, from the prototype.
 * `oneway` is a one-way main road: no line in its middle. `paving` is a town's footway.
 * Then the ballast beds of the tracks: standard and narrow gauge, each without and with a rack.
 */
export const SURFACES = { main: 4.5, road: 3.0, gravel: 1.75, dirt: 1.5, grass: 1.25, path: 0.6, oneway: 4.5, paving: 1.25, rail: 1.9, narrow: 1.5, rack: 1.9, 'narrow-rack': 1.5 } as const;
type Surface = keyof typeof SURFACES;
const codeOf = (surface: Surface) => Object.keys(SURFACES).indexOf(surface) + 1;
/** The flat shoulder past a road's edge by its surface, where it has no sidewalk. */
export const SHOULDER_M: Partial<Record<Surface, number>> = { main: 0.6, road: 0.4, gravel: 0.3, dirt: 0.25, grass: 0.2, oneway: 0.6 };
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
 * building is near, at the height of its edge. A rural road has a ditch `ditch` metres deep beside its cut.
 */
export interface Profile { tol: number; reach: number; least: number; cut: number; fill: number; tilt: number; walk: number; ditch: number }
/** A river's water lies flat on its core; banks of 27° near the water stay under its ribbon's edge. */
const RIVER_BANKS: Profile = { tol: 0.02, reach: 15, least: 2, cut: 0.5, fill: 0.5, tilt: 0, walk: 0, ditch: 0 };
/** One row per `highway` class of the road and path kinds, from the class rules that the road solver of the bake reads too, and the rivers. */
export const PROFILES: Record<string, Profile> = {
    ...Object.fromEntries(Object.entries(CLASS_RULES).map(([name, { tol, reach, least, cut, fill, tilt, ditch_m }]) => [name, { tol, reach, least, cut, fill, tilt, walk: SIDEWALK_M[name] ?? 0, ditch: ditch_m }])),
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
/** The table as stamp-core.wgsl reads it: PROFILE_ROWS rows of 8 floats, and the shoulder of each of the SURFACE_CODES surface codes. */
export const PROFILE_ROWS = 32, PROFILE_FLOATS = 8, SURFACE_CODES = 16;
export function profileTable(): Float32Array<ArrayBuffer> {
    const rows = Object.values(PROFILES), table = new Float32Array(PROFILE_ROWS * PROFILE_FLOATS + SURFACE_CODES);
    if (rows.length > PROFILE_ROWS) throw new Error(`stamp-core.wgsl holds ${PROFILE_ROWS} profiles, not ${rows.length}.`);
    rows.forEach(({ tol, reach, least, cut, fill, tilt, walk, ditch }, i) => table.set([tol, reach, least, cut, fill, tilt, walk, ditch], PROFILE_FLOATS * i));
    for (const [surface, metres] of Object.entries(SHOULDER_M)) table[PROFILE_ROWS * PROFILE_FLOATS + codeOf(surface as Surface)] = metres;
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

/**
 * How traffic runs on a one-way road: 1 along its line, -1 against it, 0 by turns; none on a two-way road.
 * A motorway and a roundabout are one-way unless OSM says otherwise.
 */
function onewayOf({ highway = '', oneway = '', junction = '' }: Record<string, string>): Line['oneway'] {
    if (!oneway) return ['motorway', 'motorway_link'].includes(highway) || junction === 'roundabout' ? 1 : undefined;
    return ({ yes: 1, true: 1, 1: 1, '-1': -1, reverse: -1, reversible: 0, alternating: 0 } as Record<string, 1 | -1 | 0>)[oneway];
}

/** The prototype's road surface of a highway (`road_surface` in its bake); `town` where a building is near. */
export function surfaceOf(tags: Record<string, string>, town = false): Surface {
    const { highway = '', surface = '', tracktype = '' } = tags;
    // Traffic that takes turns, or changes its direction, keeps no line in the middle either.
    const main = onewayOf(tags) !== undefined ? 'oneway' : 'main';
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

/** The classes of road that wear lines, and the OSM surfaces that hide a road's class. */
export const MOTORWAYS = ['motorway', 'motorway_link', 'trunk', 'trunk_link'], MAINS = ['primary', 'primary_link', 'secondary', 'secondary_link', 'tertiary', 'tertiary_link'];
export const SETTS = ['sett', 'cobblestone', 'unhewn_cobblestone', 'stone'];
/** Whether a road wears lines: one of those classes on asphalt. Its corridor (lookOf of roads/section.ts) and its paint read this one fact. */
export function marked(line: Line): boolean {
    const tag = line.surface ?? '', surface = Object.keys(SURFACES)[(line.code & 15) - 1] as Surface;
    return [...MOTORWAYS, ...MAINS].includes(CLASSES[line.profile]) && PAVED_ROADS.includes(surface) && !SETTS.includes(tag) && tag !== 'paving_stones' && !tag.startsWith('concrete');
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
 * A line; `anchors` marks its vertices of the tile among the points of its bends, and `plan` holds its
 * alignment per point where the tile has one. `tilt` holds, per
 * segment of a road, the slopes that its core takes across it from its start and from its end, as
 * `packTilt` packs them: the through road's surface near a junction, zero elsewhere. `join` marks the
 * first and the last vertex where the line runs on into the next line of the through road at a bend.
 */
export interface Line {
    kind: number; xy: number[]; z: number[]; h: number; code: number; ford: boolean; profile: number; anchors?: boolean[]; plan?: Plan; tilt?: number[]; join?: [boolean, boolean];
    /** On a road, a bridge or a tunnel, the metres along its stroke at its first vertex, and their sense along it, so that its dashes run on into the next way. */
    stroke?: { at: number; sign: 1 | -1 };
    /**
     * A bridge's deck depth, a tunnel's bore height or the height of a gallery's roof at its hill, a
     * bridge's look, the side of a gallery's hill (1 left, -1 right of the line), and whether the start and the end of a bridge or a tunnel meet the ground, as `abutments` has it.
     */
    deck?: number; style?: BridgeStyle; gallery?: 1 | -1; abut?: [boolean, boolean];
    /** A track's count of sleepers from its start at each vertex, whole at its OSM vertices; whether a contact wire hangs over it. */
    sleepers?: number[]; wire?: boolean;
    /** A town road's sides with a sidewalk per segment, LEFT and RIGHT of town.ts, as townPieces decides them. */
    sidewalks?: number[];
    /** Whether it is a roundabout's ring; how its traffic runs where it is one-way, as `onewayOf` says. */
    roundabout?: boolean; oneway?: 1 | -1 | 0;
    /** Per vertex, the ways out of it that leave its junction as a side: SIDE_BACK and SIDE_ON. */
    sideWays?: number[];
    /** The `surface` tag of its way. */
    surface?: string;
}
/** An OSM guardrail or retaining wall beside a road, in world units, with a wall's `height` tag. */
export interface Barrier { xy: number[]; wall: boolean; height?: number }
/** A lake: its level, and its rings in world units; a ring after an outer ring and before the next is its hole. */
export interface Lake { level: number; rings: number[][]; outer: boolean[] }
/** A building: the outward unit direction of the wall that faces its road, in world units. */
export interface Building { rings: number[][]; outer: boolean[]; front?: [number, number] }
/**
 * The shapes of a tile: lines, lakes and the sea with heights, the river areas, which lie on the drawn
 * surface, the pads under the platforms and the buildings.
 */
export interface Shapes { lines: Line[]; lakes: Lake[]; seas: Lake[]; areas: Omit<Lake, 'level'>[]; pads: Lake[]; buildings: Building[]; barriers: Barrier[]; crossings: { x: number; y: number }[] }

/**
 * The numbers of stamp-core.wgsl that the gates of a line, the shore samples of the audit, the edges
 * of the water mesh and the reach of the sea use. bins.test.ts holds them equal to the shader's.
 */
export const RULES = { BUFFER_M: 40, MESH_TEXELS: 4, GATE_COS: 0.70710678, SHORE_CELLS: 1.5, PORTAL_CELLS: 2, LIFT_M: 0.03, LIFT: 0.02, SHORE_SLOPE: 0.5, BED_DEPTH: 4 } as const;
/** The bed of a lake and of the sea slopes over this many metres to its depth. */
const BED_M = RULES.BED_DEPTH / RULES.SHORE_SLOPE;
/** As MASK_RANGE of stamp.wgsl: the mask reads a road this far past its edge. */
const MASK_RANGE_M = 12.75;

/** As gate_texel of stamp-core.wgsl: the texel of a level at 45° N, for the gates that decide what a level stamps. */
export const gateTexel = (level: number) => 2 * Math.PI * EARTH_RADIUS * RULES.GATE_COS / 2 ** (level + 8);
/** As sea_shows of stamp-core.wgsl: whether a level cuts the sea. */
export const seaShows = (level: number) => RULES.LIFT_M + RULES.LIFT * RULES.MESH_TEXELS * gateTexel(level) < RULES.BED_DEPTH;

export function shows({ kind, h, profile }: Line, texel: number): boolean {
    const limit = RULES.BUFFER_M - 2 * texel - 1, { reach, least } = Object.values(PROFILES)[profile];
    if (kind === STREAM) return false;
    if (kind === PATH) return 2 * h >= texel && h + reach <= limit;
    return 2 * h >= texel && h + least <= limit;
}

/** Whether a crossing node's tags say that stripes lie on the road: `crossing:markings` decides, else `crossing`. A signal says nothing of its marks. */
export function zebraTagged(tags: Record<string, string>): boolean {
    const markings = tags['crossing:markings'];
    return markings ? /^(zebra|ladder|yes)/.test(markings) : ['zebra', 'marked', 'uncontrolled'].includes(tags.crossing);
}

/** The shapes of each decoded tile, made once: the stamps and the water of each of its nodes read them. */
const shapes = new WeakMap<Feature[], Shapes & { tile: string }>();

/** The roads, paths, rivers, lakes and sea of a tile that have heights, its river areas and its buildings, in world units. */
export function features(list: Feature[], tile: Node): Shapes {
    const name = `${tile.level}/${tile.x}/${tile.y}`;
    let known = shapes.get(list);
    if (known?.tile !== name) shapes.set(list, (known = { tile: name, ...shapesOf(list, tile) }));
    return known;
}

/** The metres along a part's stroke at its vertex `start`, which only a missing height lets differ from the first. */
function strokeAt({ along, backward }: NonNullable<FeaturePart['plan']>, xy: ArrayLike<number>, start: number, world: (xy: ArrayLike<number>, v: number) => number[]): Line['stroke'] {
    let metres = 0;
    for (let v = 0; v < start; v++) {
        const [ax, ay] = world(xy, v), [bx, by] = world(xy, v + 1);
        metres += Math.hypot(bx - ax, by - ay) * metresPerUnit(by);
    }
    return backward ? { at: along - metres, sign: -1 } : { at: along + metres, sign: 1 };
}

function shapesOf(list: Feature[], tile: Node): Shapes {
    const scale = unitScale(tile.level), ox = tile.x * EXTENT, oy = tile.y * EXTENT;
    const world = (xy: ArrayLike<number>, v: number) => [(ox + xy[2 * v]) * scale, (oy + xy[2 * v + 1]) * scale];
    const curves = new Map<FeaturePart, Curve>();
    /** A part of a line feature as its line: the chords of its curve where it has an alignment, for the outer edge of its sidewalks. */
    const drawn = (part: FeaturePart, kind: string, tags: Record<string, string>): Curve | FeaturePart => {
        if (!part.plan || !part.z) return part;
        const edge = halfOf(kind, tags) + sidewalkOf(tags, surfaceOf(tags));
        // A paved path has a hard edge, in a town too.
        const sag = Object.hasOwn(PATH_HALF, tags.highway) && ![...PAVED_ROADS, 'paving'].includes(surfaceOf(tags, true)) ? PATH_SAG : ROAD_SAG;
        return curves.get(part) ?? curves.set(part, curveOf(part, edge, sag, scale, y => scale * metresPerUnit((oy + y) * scale))).get(part)!;
    };
    const lines: Line[] = [], lakes: Lake[] = [], seas: Lake[] = [], areas: Shapes['areas'] = [], pads: Lake[] = [], barriers: Barrier[] = [], crossings: Shapes['crossings'] = [];
    const area = (parts: Feature['parts']) => ({ rings: parts.map(({ xy }) => Array.from({ length: xy.length / 2 }, (_, v) => world(xy, v)).flat()), outer: parts.map(part => part.outer) });
    const buildings: Building[] = list.filter(({ kind }) => kind === 'building').map(({ parts }) => area(parts));
    const town = townOf(buildings), beds = bedsOf(list, world, drawn);
    for (const [feature, { kind, tags, parts }] of list.entries()) {
        if (kind === 'lake' || kind === 'sea') {
            const level = parts[0]?.z?.[0];
            if (level === undefined || Number.isNaN(level)) continue;
            (kind === 'lake' ? lakes : seas).push({ level, ...area(parts) });
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
        if (kind === 'guard-rail' || kind === 'retaining-wall') {
            for (const { xy } of parts) barriers.push({ xy: Array.from({ length: xy.length / 2 }, (_, v) => world(xy, v)).flat(), wall: kind === 'retaining-wall', height: parseFloat(tags.height) || undefined });
            continue;
        }
        if (kind === 'crossing') {
            if (zebraTagged(tags)) for (const { xy } of parts) crossings.push({ x: world(xy, 0)[0], y: world(xy, 0)[1] });
            continue;
        }
        const river = kind === 'river' || kind === 'stream';
        if (!river && !['main-road', 'road', 'path', 'railway', 'tunnel', 'gallery'].includes(kind)) continue;
        // A passage through a building is a road: the building stands over it.
        const bore = kind === 'tunnel' && tags.tunnel !== 'building_passage', path = Object.hasOwn(PATH_HALF, tags.highway);
        const bridge = !river && !bore && !GROUND.includes(tags.bridge ?? 'no'), track = isTrack(tags);
        const surface = track ? trackSurface(tags) : surfaceOf(tags), h = halfOf(kind, tags), code = river ? 0 : Object.keys(SURFACES).indexOf(surface) + 1;
        const profile = profileOf(kind === 'tunnel' || kind === 'gallery' ? (path ? 'path' : 'road') : kind, classOf(tags));
        const type = kind === 'river' ? RIVER : kind === 'stream' ? STREAM : bridge ? BRIDGE : bore ? TUNNEL : kind === 'railway' ? RAIL : kind === 'path' || path ? PATH : ROAD;
        const deck = bridge ? parseFloat(tags.deck ?? '') || DECK_M : bore ? (track ? BORE_RAIL_M : path ? BORE_PATH_M : BORE_M) : kind === 'gallery' ? (track ? ROOF_RAIL_M : path ? ROOF_PATH_M : ROOF_M) : undefined;
        const wire = track && !bore && ['contact_line', 'yes'].includes(tags.electrified) && tags.railway !== 'funicular';
        const style: BridgeStyle | undefined = !bridge ? undefined : tags.bridge === 'covered' ? 'covered' : CABLES.includes(tags['bridge:structure']) ? 'cable' : 'beam';
        // In a town, where a building is near a piece of it: a road has a sidewalk on that side, and a footway is paved.
        const sidewalk = type === ROAD && kind !== 'gallery' ? sidewalkOf(tags, surface) : 0;
        const paving = type === PATH && kind !== 'gallery' && surfaceOf(tags, true) !== surface;
        for (const part of parts) {
            const curve = drawn(part, kind, tags), { xy, z } = curve, sideWaysOf = sidesAlong(part, world);
            if (!z) continue;
            // A line splits where a vertex has no height: a missing height is unknown, never 0 m. A footway splits
            // where it enters or leaves a town.
            let run: number[] = [], heights: number[] = [];
            const flush = (end: number) => {
                if (heights.length >= 2) {
                    const start = end - heights.length;
                    const whole: Shape = 'anchors' in curve ? { xy: run, z: heights, anchors: curve.anchors.slice(start, end), plan: planPart(curve.plan, start, end) } : smoothed(run, heights);
                    const [shape, sides] = sidewalk || paving ? townPieces(whole, town, sidewalk ? clearOf(beds, feature, h, sidewalk) : undefined) : [whole, []];
                    const n = shape.z.length, paved = (v: number) => paving && sides[v] !== 0, sideWays = sideWaysOf?.(shape.xy);
                    const stroke = part.plan && (type === ROAD || type === BRIDGE || type === TUNNEL) ? strokeAt(part.plan, xy, start, world) : undefined;
                    for (let from = 0, to = 1; to < n; to++) {
                        if (to < n - 1 && paved(to) === paved(from)) continue;
                        const line: Line = {
                            kind: type, xy: shape.xy.slice(2 * from, 2 * to + 2), z: shape.z.slice(from, to + 1), anchors: shape.anchors.slice(from, to + 1), plan: shape.plan && planPart(shape.plan, from, to + 1),
                            h: paved(from) ? halfWidth(tags, 'paving') : h, code: paved(from) ? PAVING_CODE : code, ford: !!tags.ford && tags.ford !== 'no', profile, deck, style,
                            gallery: kind === 'gallery' ? (tags.uphill === 'right' ? -1 : 1) : undefined, wire, sidewalks: sidewalk ? sides.slice(from, to) : undefined, sideWays: sideWays?.slice(from, to + 1), surface: tags.surface, roundabout: tags.junction === 'roundabout' || undefined, oneway: onewayOf(tags), stroke,
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
                if (Number.isNaN(z[v])) flush(v);
                else {
                    run.push(...world(xy, v));
                    heights.push(z[v]);
                }
            }
            flush(z.length);
        }
    }
    junctionPlanes(lines);
    abutments(lines);
    faceRoads(lines, buildings, town);
    return { lines, lakes, seas, areas, pads, buildings, barriers, crossings };
}

/**
 * What gives each line of a part its `sideWays`, if the part has sides. A way can pass a point twice, so
 * the vertices of the part with a height are found in their order along the line, not by their place.
 */
function sidesAlong({ xy, z, plan }: Feature['parts'][number], world: (xy: ArrayLike<number>, v: number) => number[]): ((shape: number[]) => number[]) | undefined {
    const side = plan?.side;
    if (!z || !side?.some(Boolean)) return undefined;
    // A vertex with no known height beside it starts no line, so no shape reaches it.
    const has = (v: number) => v >= 0 && v < z.length && !Number.isNaN(z[v]);
    const known = Array.from(z.keys()).filter(v => has(v) && (has(v - 1) || has(v + 1)));
    let k = 0;
    return shape => Array.from({ length: shape.length / 2 }, (_, s) => {
        if (k === known.length) return 0;
        const [x, y] = world(xy, known[k]);
        return x === shape[2 * s] && y === shape[2 * s + 1] ? side[known[k++]] : 0;
    });
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

/** A road's face lies this far over the stamped core, whose tolerance is 2 cm. */
export const LIFT_M = 0.03;
/** A town's road and footway decide their sidewalks and paving in pieces about this long. */
const PIECE_M = 8;

/**
 * A rounded line with the sides of each segment that have a building within BUILT_M, and that `clear`
 * keeps where it is given: of each piece of about PIECE_M, as its segment's own length places them, so
 * each tile that holds the segment decides alike. A segment is cut where the sides of its pieces
 * change; the cuts keep its shape.
 */
function townPieces({ xy, z, anchors, plan }: Shape, town: Town, clear?: Clear): [Shape, number[]] {
    const out: Shape = { xy: [xy[0], xy[1]], z: [z[0]], anchors: [anchors[0]], plan: plan && planPart(plan, 0, 1) }, sides: number[] = [];
    for (let v = 0; v + 1 < z.length; v++) {
        const [ax, ay, bx, by] = xy.slice(2 * v, 2 * v + 4), dx = bx - ax, dy = by - ay, unit = metresPerUnit(ay), count = Math.max(1, Math.ceil(Math.hypot(dx, dy) * unit / PIECE_M));
        const pieces = Array.from({ length: count }, (_, k) => {
            const x = ax + dx * (k + 0.5) / count, y = ay + dy * (k + 0.5) / count;
            if (!builtNear(town, x, y, BUILT_M / unit)) return 0;
            return clear ? (clear(x, y, dx, dy, LEFT) ? LEFT : 0) | (clear(x, y, dx, dy, RIGHT) ? RIGHT : 0) : LEFT | RIGHT;
        });
        for (let k = 1; k <= count; k++) {
            if (k < count && pieces[k] === pieces[k - 1]) continue;
            sides.push(pieces[k - 1]);
            out.xy.push(k === count ? bx : Math.round(ax + dx * k / count), k === count ? by : Math.round(ay + dy * k / count));
            out.z.push(z[v] + (z[v + 1] - z[v]) * k / count);
            out.anchors.push(k === count && anchors[v + 1]);
            if (plan) addPlan(out.plan!, plan, v, k / count);
        }
    }
    return [out, sides];
}

/**
 * The beds that the stamp holds on the ground, of a tile's roads, paths, tracks and waterways: their
 * segments in a grid of their boxes grown by their half-widths, the features at each vertex and each
 * feature's vertices.
 */
interface Beds extends Grid { segments: { feature: number; h: number; xy: number[] }[]; vertices: Map<string, number[]>; points: number[][][] }
/** Whether the sidewalk on a side of a street's piece through x, y along dx, dy keeps out of the beds. */
type Clear = (x: number, y: number, dx: number, dy: number, side: number) => boolean;

/** The half-width of a line feature in metres: a waterway's, a track's bed or a road's. */
function halfOf(kind: string, tags: Record<string, string>): number {
    const width = parseFloat(tags.width ?? '');
    if (kind === 'river' || kind === 'stream') return Number.isFinite(width) ? width / 2 : WATERWAYS[tags.waterway] ?? 1;
    return isTrack(tags) ? SURFACES[trackSurface(tags)] : halfWidth(tags, surfaceOf(tags));
}

function bedsOf(list: Feature[], world: (xy: ArrayLike<number>, v: number) => number[], drawn: (part: FeaturePart, kind: string, tags: Record<string, string>) => Curve | FeaturePart): Beds {
    const segments: Beds['segments'] = [], vertices = new Map<string, number[]>(), points: number[][][] = [];
    list.forEach(({ kind, tags, parts }, feature) => {
        // A bridge's and a tunnel's beds are off the ground.
        if (!['main-road', 'road', 'path', 'railway', 'river', 'stream'].includes(kind) || !GROUND.includes(tags.bridge ?? 'no')) return;
        const h = halfOf(kind, tags);
        points[feature] = [];
        // As in the stamp, a part or a segment without heights, as a culvert's, holds no ground.
        for (const part of parts) {
            const { xy, z } = drawn(part, kind, tags);
            if (!z) continue;
            const at = Array.from({ length: xy.length / 2 }, (_, v) => world(xy, v));
            for (const point of at) {
                const key = `${point[0]},${point[1]}`;
                points[feature].push(point);
                (vertices.get(key) ?? vertices.set(key, []).get(key)!).push(feature);
            }
            for (let v = 0; v + 1 < at.length; v++) if (!Number.isNaN(z[v]) && !Number.isNaN(z[v + 1])) segments.push({ feature, h, xy: [...at[v], ...at[v + 1]] });
        }
    });
    const boxes = segments.map(({ h, xy: [ax, ay, bx, by] }) => {
        const r = h / metresPerUnit(ay);
        return [Math.min(ax, bx) - r, Math.min(ay, by) - r, Math.max(ax, bx) + r, Math.max(ay, by) + r];
    });
    return { ...gridOf(boxes), segments, vertices, points };
}

/**
 * A line that meets a street at a vertex does not count within this many metres of that vertex, a little
 * past the end of the corner's arc, at most 10 m along each road (TANGENT_M of corners.ts): the corner
 * shapes the mouth of a side street, a path or a level crossing. The vertex and the beds that the test
 * reads lie within BUILT_M of the piece's middle, as its buildings do, for beds up to about 14 m either
 * side of their line: each tile decides alike.
 */
const JOIN_M = 12;

/**
 * The sidewalk test of a street of half-width h with sidewalks `walk` metres wide: the sidewalk lies
 * outside the bed of every other line, but near the vertices where those lines meet the street.
 */
function clearOf(beds: Beds, feature: number, h: number, walk: number): Clear {
    const joins = (beds.points[feature] ?? []).map(([x, y]) => ({ x, y, lines: beds.vertices.get(`${x},${y}`)!.filter(f => f !== feature) })).filter(({ lines }) => lines.length);
    return (x, y, dx, dy, side) => {
        const unit = metresPerUnit(y), l = Math.hypot(dx, dy) * unit || 1, out = (side === LEFT ? 1 : -1) * (h + walk / 2) / l;
        // Left of the way, whose y runs south.
        const mx = x + dy * out, my = y - dx * out;
        const joined = (f: number) => joins.some(join => join.lines.includes(f) && Math.hypot(join.x - mx, join.y - my) * unit < JOIN_M);
        let clear = true;
        near(beds, mx, my, mx, my, walk / 2 / unit, b => {
            const { feature: f, h: bed, xy: [ax, ay, bx, by] } = beds.segments[b];
            if (!clear || f === feature || joined(f)) return;
            const t = along(ax, ay, bx, by, mx, my);
            clear = Math.hypot(ax + t * (bx - ax) - mx, ay + t * (by - ay) - my) * unit >= bed + walk / 2;
        });
        return clear;
    };
}

/** The shortest wall that is a front; a road or a path this near a building reaches it; a wall faces a point within 60° of its normal. */
const WALL_M = 2, REACHED_M = 2, FACING_COS = 0.5;

/**
 * The fronts of the houses: a building whose nearest road on the ground comes within BUILT_M, and no road or
 * path within REACHED_M, fronts that road with its nearest wall that faces it. Only the house and the lines
 * near it decide, so each tile that holds them all draws it alike.
 */
function faceRoads(lines: Line[], buildings: Building[], town: Town): void {
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
    buildings.forEach((building, b) => {
        if (nearest[b].gap >= REACHED_M && nearest[b].road) building.front = frontOf(building, nearest[b].road);
    });
}

/** The outward direction of a building's nearest wall that faces a road; none where no wall faces it. */
function frontOf(building: Building, road: Line): [number, number] | undefined {
    const ring = building.rings[0], n = ring.length / 2, unit = metresPerUnit(ring[1]);
    // The ring's outside lies right of each edge (y south) where its area is positive. About its first vertex: products of world units lose float64 bits.
    let twice = 0;
    for (let i = 0, j = n - 1; i < n; j = i++) twice += (ring[2 * j] - ring[0]) * (ring[2 * i + 1] - ring[1]) - (ring[2 * i] - ring[0]) * (ring[2 * j + 1] - ring[1]);
    let front: [number, number] | undefined, least = Infinity;
    for (let i = 0; i < n; i++) {
        const k = (i + 1) % n, length = Math.hypot(ring[2 * k] - ring[2 * i], ring[2 * k + 1] - ring[2 * i + 1]);
        if (length * unit < WALL_M) continue;
        const mx = (ring[2 * i] + ring[2 * k]) / 2, my = (ring[2 * i + 1] + ring[2 * k + 1]) / 2;
        const nx = Math.sign(twice) * (ring[2 * k + 1] - ring[2 * i + 1]) / length, ny = Math.sign(twice) * (ring[2 * i] - ring[2 * k]) / length;
        const { x, y } = closest(road, mx, my), d = Math.hypot(x - mx, y - my);
        if ((x - mx) * nx + (y - my) * ny >= FACING_COS * d && d < least) [front, least] = [[nx, ny], d];
    }
    return front;
}

/** The point of a line nearest to x, y. */
function closest(line: Line, x: number, y: number): { x: number; y: number } {
    let best = { d: Infinity, x, y };
    for (let v = 0; v + 1 < line.z.length; v++) {
        const ax = line.xy[2 * v], ay = line.xy[2 * v + 1], dx = line.xy[2 * v + 2] - ax, dy = line.xy[2 * v + 3] - ay;
        const t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1))), px = ax + t * dx, py = ay + t * dy, d = (px - x) ** 2 + (py - y) ** 2;
        if (d < best.d) best = { d, x: px, y: py };
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

/** A way out of a vertex of a line, step 1 or -1 along it. The bake marks it a side of its junction, or it runs through. */
export interface JunctionWay { line: Line; v: number; step: 1 | -1; side: boolean }

/**
 * The ways out of each vertex that lines of roads and tracks share. Of a junction of three ways or more, the
 * bake marks each way but its through pair as a side. Two ways run on, and a tile below zoom 13 holds no sides.
 */
export function junctionWays(lines: Line[]): JunctionWay[][] {
    const at = new Map<string, JunctionWay[]>();
    for (const line of cores(lines)) {
        if (isPath(line)) continue;
        line.z.forEach((_, v) => {
            if (!(line.anchors?.[v] ?? true)) return;
            const name = `${line.xy[2 * v]},${line.xy[2 * v + 1]}`, bits = line.sideWays?.[v] ?? 0, ways = at.get(name) ?? at.set(name, []).get(name)!;
            for (const [step, bit] of [[-1, SIDE_BACK], [1, SIDE_ON]] as const) if (v + step >= 0 && v + step < line.z.length) ways.push({ line, v, step, side: !!(bits & bit) });
        });
    }
    return [...at.values()].filter(ways => ways.some(({ line, v }) => line !== ways[0].line || v !== ways[0].v));
}

/** The two ways of a through road that OSM splits at a junction: both end there. */
export function splitThrough(ways: JunctionWay[]): JunctionWay[] | undefined {
    const through = ways.filter(({ side }) => !side);
    return through.length === 2 && through.every(({ line, v }) => v === 0 || v === line.z.length - 1) ? through : undefined;
}

/**
 * The plane of the through pair at each junction of roads, on the side roads: the road solver of the
 * bake puts a side road's first metres on the surface of a through road, and its core takes the slope
 * of the through way that it leans along most across it. Only the vertices and heights at a junction
 * decide, so every tile that holds a junction gets the same planes.
 */
function junctionPlanes(lines: Line[]): void {
    for (const line of lines) if (line.kind === ROAD) line.tilt = new Array(2 * (line.z.length - 1)).fill(0);
    const next = (line: Line, v: number, step: number) => {
        let w = v + step;
        while (w > 0 && w < line.z.length - 1 && !(line.anchors?.[w] ?? true)) w += step;
        return w;
    };
    const metres = (line: Line, a: number, b: number) => Math.hypot(line.xy[2 * b] - line.xy[2 * a], line.xy[2 * b + 1] - line.xy[2 * a + 1]) * metresPerUnit(line.xy[2 * a + 1]);
    const dot = (p: number[], q: number[]) => p[0] * q[0] + p[1] * q[1];
    for (const meeting of junctionWays(lines)) {
        const ways = meeting.map(way => {
            const { line, v, step } = way, w = next(line, v, step), length = Math.hypot(line.xy[2 * w] - line.xy[2 * v], line.xy[2 * w + 1] - line.xy[2 * v + 1]) || 1;
            if (line.plan) return { ...way, dir: [step * Math.cos(line.plan.tangent[v]), step * Math.sin(line.plan.tangent[v])], grade: step * line.plan.grade[v] };
            return { ...way, dir: [(line.xy[2 * w] - line.xy[2 * v]) / length, (line.xy[2 * w + 1] - line.xy[2 * v + 1]) / length], grade: (line.z[w] - line.z[v]) / metres(line, v, w) };
        });
        const split = splitThrough(ways) as typeof ways | undefined, through = ways.filter(({ side }) => !side);
        if (split?.every(({ line }) => line.kind === ROAD)) {
            // Where OSM splits the through road at a bend, the round cap of one end paints the outer wedge.
            const [cap] = [...split].sort((p, q) => p.dir[0] - q.dir[0] || p.dir[1] - q.dir[1]);
            (cap.line.join ??= [false, false])[cap.v === 0 ? 0 : 1] = true;
        }
        if (!through.length) continue;
        const reach = Math.max(APPROACH_M, ...through.map(({ line }) => line.h)) + WARP_M;
        for (const side of ways) {
            if (!side.side || side.line.kind !== ROAD) continue;
            const lean = through.reduce((most, way) => (dot(way.dir, side.dir) > dot(most.dir, side.dir) ? way : most));
            // The through road's bank tilts its plane: a rise to the left of its line's direction.
            const bank = lean.step * (lean.line.plan?.bank[lean.v] ?? 0);
            const plane = [lean.grade * lean.dir[0] + bank * lean.dir[1], lean.grade * lean.dir[1] - bank * lean.dir[0]], { line, step } = side, tilt = line.tilt!;
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

/** The rise per metre to the right of segment v of a line, `sa` metres from its start and `sb` from its end, as `along` in stamp-core.wgsl reads it. */
export function crossSlope(line: Line, v: number, sa: number, sb: number): number {
    const bank = line.plan ? line.plan.bank[v] + (line.plan.bank[v + 1] - line.plan.bank[v]) * Math.min(1, Math.max(0, sa / Math.max(sa + sb, 1e-6))) : 0;
    return (line.tilt ? tiltAt(line.tilt[2 * v], sa) + tiltAt(line.tilt[2 * v + 1], sb) : 0) - bank;
}

/** A line as `smoothed` and `townPieces` hand it on. */
type Shape = Pick<Line, 'xy' | 'z' | 'plan'> & { anchors: boolean[] };

/** The most that a chord of a rounded bend leaves the curve, in world units (about 10 cm), and the most chords per segment. */
const SAGITTA = 4, CHORDS = 16;

/**
 * A line with no alignment through its vertices with its bends rounded: each segment between two other segments follows
 * the centripetal Catmull-Rom curve through the four vertices about it, in chords of whole world
 * units, with the height linear along it. The first and last segments stay straight, also of a
 * closed way: the feature tiles cut a ring into open pieces, so every line is open. They cut a line
 * one vertex past the segments that meet a tile's buffer, so every segment that a tile's nodes stamp
 * gets the same curve in each tile that holds it; and every vertex stays, so junctions stay where they are.
 */
export function smoothed(xy: number[], z: number[]): Shape {
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

/**
 * Junctions: vertices that two road lines meet at, or that one line passes twice (a ring's start), with the
 * widest half-width there and the count of ways out: two where OSM splits a road.
 */
export function junctions(lines: Line[]): Map<string, { x: number; y: number; h: number; ways: number }> {
    const seen = new Map<string, { x: number; y: number; h: number; ways: number; count: number }>();
    for (const line of lines) {
        for (let v = 0; v < line.z.length; v++) {
            const x = line.xy[2 * v], y = line.xy[2 * v + 1], key = `${x},${y}`;
            const entry = seen.get(key) ?? { x, y, h: 0, ways: 0, count: 0 };
            entry.count++;
            entry.ways += (v > 0 ? 1 : 0) + (v + 1 < line.z.length ? 1 : 0);
            entry.h = Math.max(entry.h, line.h);
            seen.set(key, entry);
        }
    }
    return new Map([...seen].filter(([, { count }]) => count > 1));
}

/**
 * The unit mitre normal at vertex v of a line: its tangent where it has an alignment, else along the line
 * at an end, the bisector of the two segments inside. At an OSM vertex the segments run to the OSM vertices
 * about it, not to the points of a rounded bend: a tile rounds only the segments with a vertex on both
 * sides, so this is the same in every tile.
 */
export function mitre(line: Line, v: number): [number, number] {
    if (line.plan) return [Math.cos(line.plan.tangent[v]), Math.sin(line.plan.tangent[v])];
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

/** As crosses() of stamp-core.wgsl: whether the way from origin o to p crosses edge a-b, with p moved by ε(1, ε). */
export function crosses([ox, oy]: number[], [px, py]: number[], [ax, ay, bx, by]: number[]): boolean {
    const left = (ux: number, uy: number, wx: number, wy: number) => ux * wy > uy * wx || (ux * wy === uy * wx && (uy < 0 || (uy === 0 && ux > 0)));
    const one = (bx - ax) * (oy - ay) - (by - ay) * (ox - ax) > 0;
    return left(ax - ox, ay - oy, px - ox, py - oy) !== left(bx - ox, by - oy, px - ox, py - oy) && one !== left(bx - ax, by - ay, px - ax, py - ay);
}

/**
 * The cell of world point (x, y) in the stamp data of a node, as stamp_at finds it: its entries and the
 * origin of its lake crossings.
 */
export function cellAt(data: Uint32Array, node: Node, x: number, y: number): { entries: Uint32Array; origin: [number, number] } {
    const shift = 22 - node.level, cell = CELL * 2 ** shift, x0 = node.x * PAGE * 2 ** shift - cell, y0 = node.y * PAGE * 2 ** shift - cell;
    const index = (v: number, base: number) => Math.min(GRID - 1, Math.max(0, Math.floor((v - base) / cell)));
    const c = index(y, y0) * GRID + index(x, x0), offset = data[HEAD + c];
    const held = (v: number, base: number) => Math.min(Math.max(v, base + cell), base + cell + PAGE * 2 ** shift - 1);
    const origin: [number, number] = [held(x0 + (c % GRID + 0.5) * cell, x0) + ((offset >>> 24) & 15) - 8, held(y0 + (Math.floor(c / GRID) + 0.5) * cell, y0) + (offset >>> 28) - 8];
    return { entries: data.subarray(HEAD + OFFSETS + (offset & 0xffffff), HEAD + OFFSETS + (data[HEAD + c + 1] & 0xffffff)), origin };
}

/** The highest level of the lakes and the sea that hold world point (x, y) in the stamp data of a node, as `water` of stamp_at; -Infinity in none. */
export function waterAt(data: Uint32Array, node: Node, x: number, y: number): number {
    const { entries, origin } = cellAt(data, node, x, y), records = HEAD + OFFSETS + data[1], levels = new Float32Array(data.buffer, data.byteOffset);
    let crossed = false, level = -Infinity;
    for (const entry of entries) {
        const r = records + RECORD * (entry & ~INSIDE), kind = data[r + 4];
        if (kind === EDGE) {
            crossed = crossed !== crosses(origin, [x, y], Array.from(data.subarray(r, r + 4), w => w | 0));
            continue;
        }
        if ((kind === LAKE || kind === SEA) && ((entry & INSIDE) !== 0) !== crossed) level = Math.max(level, levels[r + 6]);
        crossed = false;
    }
    return level;
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
    /** A road, a path, a lake or the sea is near: the node may stamp heights. */
    heights: boolean;
    /** A line, an area or a shore lies in the node: on a near level it holds a feature mask. */
    mask: boolean;
}

/**
 * The stamp data of a node from the features of its feature tile. Every primitive goes to each cell
 * that it can act on, so the shader sees every primitive that can reach a texel. A cell keeps the
 * order of stamp-core.wgsl: a lake's edges before its marker, a line's segments together.
 */
export function nodeStamps(list: Feature[], tile: Node, node: Node): NodeStamps {
    const { lines, lakes, seas, pads, areas, buildings } = features(list, tile);
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
    /** Two numbers within ±1 as unpack2x16snorm reads them. */
    const pair = (a: number, b: number) => (Math.round(a * 32767) & 0xffff) | (Math.round(b * 32767) << 16);

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
    if (lines.length > 0x10000) throw new Error(`nodeStamps: ${lines.length} lines pass the 16-bit index`);
    lines.forEach((line, id) => {
        const stamps = shows(line, gateTexel(node.level));
        if (line.kind === STREAM || (!stamps && node.level < NEAR)) return;
        // A path acts no farther than its bench, and on the near levels than the mask reads it.
        const reach = line.kind === PATH ? line.h + (node.level >= NEAR ? MASK_RANGE_M : Object.values(PROFILES)[line.profile].reach) : REACH_M;
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
            const flags = (line.kind === BRIDGE || line.kind === TUNNEL ? 0 : (line.ford ? FORD : 0) | (dead(v) ? CORE_A : 0) | (dead(v + 1) ? CORE_B : 0) | (end(v) ? CONE_A : 0) | (end(v + 1) ? CONE_B : 0)
                | (join(v) ? JOIN_A : 0) | (join(v + 1) ? JOIN_B : 0)) | (marked(line) ? MARKED : 0);
            // The deck, the bore or the roof in decimetres; the top bit marks a gallery whose hill is on its right.
            const deck = Math.min(line.gallery ? 127 : 255, Math.round(10 * (line.deck ?? 0))) | (line.gallery === -1 ? 128 : 0);
            // A bridge's or a tunnel's metres from its ends where they meet the ground, a gallery's from its ends, a track's sleepers; else a road's tilts.
            const ends = line.abut ? [line.abut[0] ? along[v] : 1e30, line.abut[1] ? along[n - 1] - along[v + 1] : 1e30].map(f)
                : line.gallery ? [along[v], along[n - 1] - along[v + 1]].map(f) : line.sleepers ? [line.sleepers[v], line.sleepers[v + 1]].map(f)
                    : [line.tilt?.[2 * v] ?? 0, line.tilt?.[2 * v + 1] ?? 0];
            // The surface code in the low 4 bits, the sides with a sidewalk above it. Beside the line's index, in
            // steps of 2 cm, how far its edge lies out at the start and the end, where it tapers to a partner's.
            const taper = (w: number) => Math.max(-127, Math.min(127, Math.round((halfAt(line, w, partners) - line.h) / 0.02))) & 0xff;
            // The mitres of the two ends, then the rise of the bank to the right at each. One word stays free.
            const rise = (w: number) => -(line.plan?.bank[w] ?? 0);
            const at = record([ax, ay, bx, by, line.kind, line.code | ((line.sidewalks?.[v] ?? 0) << 4) | (flags << 8) | (line.profile << 16) | (deck << 24), f(line.z[v]), f(line.z[v + 1]), f(line.h),
                pair(mx, my), pair(nx, ny), pair(rise(v), rise(v + 1)), 0, id | (taper(v) << 16) | (taper(v + 1) << 24), ...ends]);
            for (const c of touched) cells[2][c].push(at);
            heights ||= stamps;
        }
    });
    // The stamps of a node read only the cells inside its apron.
    const inner = (c: number) => c % GRID > 0 && c % GRID < GRID - 1 && c >= GRID && c < GRID * (GRID - 1);
    let mask = [cells[0], cells[2]].some(lists => lists.some((list, c) => inner(c) && list.length > 0));
    // Per lake, sea polygon, pad and area near the node, its edges in each cell that they reach.
    const outlines = [...lakes.map(lake => ({ lake, kind: LAKE })), ...(seaShows(node.level) ? seas.map(lake => ({ lake, kind: SEA })) : []),
        ...(node.level >= PAD_LEVEL ? pads.map(lake => ({ lake, kind: PAD })) : []),
        ...(node.level >= NEAR ? [...areas, ...buildings].map(area => ({ lake: { level: 0, ...area }, kind: AREA })) : [])];
    const all = Array.from({ length: GRID * GRID }, (_, c) => c);
    const near = outlines.flatMap(({ lake, kind }) => {
        // Not a spread: a sea polygon has more vertices than a call takes arguments.
        const box = [Infinity, Infinity, -Infinity, -Infinity];
        for (const ring of lake.rings) for (let i = 0; i < ring.length; i += 2) {
            box[0] = Math.min(box[0], ring[i]);
            box[1] = Math.min(box[1], ring[i + 1]);
            box[2] = Math.max(box[2], ring[i]);
            box[3] = Math.max(box[3], ring[i + 1]);
        }
        // An area acts only inside it: its edges go to the cells that they cross, so a village's cells stay
        // short. The sea acts no farther than the end of its bed's slope from its coast, so its long coasts
        // stay short too: BED_M past sea_band of stamp-core.wgsl, in units at most a shore band and the
        // buffer less BED_M, a metre and two texels.
        const band = Math.min(RULES.SHORE_CELLS * RULES.MESH_TEXELS * 2 ** shift, Math.max(0, (RULES.BUFFER_M - 1 - BED_M) * units - 2 * 2 ** shift));
        const metres = kind === AREA ? 0 : kind === SEA ? BED_M + band / units : REACH_M, reach = metres * units;
        if (box[2] < x0 - reach || box[0] > x0 + span + reach || box[3] < y0 - reach || box[1] > y0 + span + reach) return [];
        const edges = new Map<number, number[]>();
        for (const ring of lake.rings) for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
            const [ax, ay, bx, by] = [ring[j], ring[j + 1], ring[i], ring[i + 1]];
            const touched = within([Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)], metres);
            if (!touched.length) continue;
            const at = record([ax, ay, bx, by, EDGE]);
            for (const c of touched) (edges.get(c) ?? edges.set(c, []).get(c)!).push(at);
        }
        const cells = kind === AREA ? within(box, 0) : all;
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
        // Without an edge in the node, every cell lies on the side of the first.
        const marker = record([0, 0, 0, 0, kind, 0, f(lake.level)]);
        const side = edges.size || !reached.length ? undefined : insideRings(lake.rings, ...origins[reached[0]]);
        let used = false;
        for (const c of reached) {
            const inside = side ?? insideRings(lake.rings, ...origins[c]);
            if (!inside && !edges.has(c)) continue;
            cells[1][c].push(...(edges.get(c) ?? []), marker | (inside ? INSIDE : 0));
            used ||= inner(c);
            // A lake takes its inside, where nothing grows; the open sea needs no mask.
            mask ||= inner(c) && (edges.has(c) || kind === AREA || kind === LAKE);
        }
        // The box of a sea polygon covers much land, where it stamps nothing.
        heights ||= used && kind !== AREA;
    }
    if (!records.length || (!heights && (!mask || node.level < NEAR))) return { data: null, heights: false, mask: false };
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
    return { data, heights, mask };
}

/** The moves of a cell's origin that the worker tries, in world units, as a signed nibble each. */
const NUDGES = [[0, 0], [1, 0], [0, 1], [1, 1], [2, 1], [1, 2], [-1, 2], [3, 1], [-2, 3], [3, -2], [4, 3], [-3, 4]];

function segmentDistance(x: number, y: number, ax: number, ay: number, bx: number, by: number): number {
    const dx = bx - ax, dy = by - ay, t = Math.min(1, Math.max(0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
    return Math.sqrt((x - ax - t * dx) ** 2 + (y - ay - t * dy) ** 2);
}
