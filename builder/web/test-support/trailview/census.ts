// The fault census of the TrailView fixture, on the CPU: the counts that the feature tiles and the corridor
// meshes decide, and the samples at which census.mjs reads the drawn terrain and the mask on the GPU.
// Each count's doc comment is its definition; the README's "Shots and flights" says the same in short.
import { readdirSync, readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import config from '../../tests/browser/trailview/fixture/config.json' with { type: 'json' };
import { worldPixel } from '../../src/lib/planner/layers/mercator';
import { UNITS } from '../../src/lib/trailview/frame/space';
import { KINDS } from '../../src/lib/trailview/kinds';
import { placed, type Build } from '../../src/lib/trailview/kinds/kind';
import { coreAt, coresOf, stepSide } from '../../src/lib/trailview/roads/cores';
import { isDeck } from '../../src/lib/trailview/roads/decks';
import { planOf, roadMesh, type RoadMesh } from '../../src/lib/trailview/roads/mesh';
import { ROAD_VERTEX, type RoadPiece } from '../../src/lib/trailview/roads/piece';
import { CF, LOOK, MAT } from '../../src/lib/trailview/roads/section';
import { between } from '../../src/lib/trailview/roads/stations';
import { decodeFeatures, DEEPEST, type Feature } from '../../src/lib/trailview/stream/features';
import { AnchoredBuilder } from '../../src/lib/trailview/structures/anchored';
import { MeshBuilder } from '../../src/lib/trailview/structures/mesh';
import { features, insideRings, metresPerUnit, PATH, RIVER, ROAD, STREAM, walkOf, type Line } from '../../src/lib/trailview/terrain/bins';
import { cornerAt } from '../../src/lib/trailview/terrain/corners';
import { CHILD, OWNER, size, type Node } from '../../src/lib/trailview/terrain/lod';
import { LEFT, RIGHT } from '../../src/lib/trailview/terrain/town';

export interface FixtureTile { tile: Node; list: Feature[]; meshes: RoadMesh[] }

const FEATURES = new URL('../../tests/browser/trailview/fixture/features/', import.meta.url);

/** The feature tiles of the fixture, each with the corridor mesh of its 16 owners. */
export function fixtureTiles(): FixtureTile[] {
    return readdirSync(new URL(`${DEEPEST}/`, FEATURES)).flatMap(x => readdirSync(new URL(`${DEEPEST}/${x}/`, FEATURES)).map(file => {
        const tile = { level: DEEPEST, x: Number(x), y: parseInt(file, 10) }, list = decodeFeatures(new Uint8Array(gunzipSync(readFileSync(new URL(`${DEEPEST}/${x}/${file}`, FEATURES)))));
        return { tile, list, meshes: Array.from({ length: 16 }, (_, i) => roadMesh(list, tile, { level: OWNER, x: tile.x * 4 + i % 4, y: tile.y * 4 + (i >> 2) })) };
    }));
}

/** A place is a square of this many metres that holds 2 or more samples of one count. As PLACE_M of census.mjs. */
const PLACE_M = 10;

/** The name of the place square that holds a world point. */
export const placeOf = (x: number, y: number) => `${Math.floor(x * metresPerUnit(y) / PLACE_M)},${Math.floor(y * metresPerUnit(y) / PLACE_M)}`;

/** The census line of a count: its samples (world x, y pairs) and its places. */
export function countLine(count: string, points: ArrayLike<number>): string {
    const squares = new Map<string, number>();
    for (let i = 0; i < points.length; i += 2) {
        const name = placeOf(points[i], points[i + 1]);
        squares.set(name, (squares.get(name) ?? 0) + 1);
    }
    return JSON.stringify({ count, samples: points.length / 2, places: [...squares.values()].filter(n => n >= 2).length });
}

/** The census leaves out this many metres at the edges of the fixture's terrain: past them, a way has no tile on one side. */
const MARGIN_M = 70;
const [west, north] = worldPixel([config.bounds[0], config.bounds[3]], UNITS), [east, south] = worldPixel([config.bounds[2], config.bounds[1]], UNITS);

/** Whether a world point lies MARGIN_M or more inside the fixture's bounds. */
function inBounds(x: number, y: number): boolean {
    const margin = MARGIN_M / metresPerUnit(y);
    return x > west + margin && x < east - margin && y > north + margin && y < south - margin;
}

/** Whether a world point counts for a tile: in bounds and in the tile's own square, not in its border, so that each tile counts its own. */
const inSquare = ({ x, y, level }: Node, px: number, py: number) => Math.floor(px / size(level)) === x && Math.floor(py / size(level)) === y && inBounds(px, py);

/** A piece's vertices as corridor.wgsl reads them: world x, y, z, the look word and the word of across and along. */
export function vertices({ x, y, vertices }: RoadPiece) {
    const steps = new Int16Array(vertices), words = new Uint32Array(vertices), floats = new Float32Array(vertices);
    return {
        count: vertices.byteLength / ROAD_VERTEX,
        x: (v: number) => x + (steps[8 * v] * 4 + ((words[4 * v + 2] >>> 28) & 3)) / 12,
        y: (v: number) => y + (steps[8 * v + 1] * 4 + (words[4 * v + 2] >>> 30)) / 12,
        z: (v: number) => floats[4 * v + 1],
        word: (v: number) => words[4 * v + 2],
        place: (v: number) => words[4 * v + 3],
    };
}

/** Whether a vertex is a band's row, whose height exists only on the GPU. */
const banded = (word: number) => (((word >>> 11) & 31) & (CF.SAMPLE | CF.MESH)) !== 0;

/** Each drawn face of the near range of an L16 piece that has no band row: the faces of a way's footprint. */
function* pieceFaces(piece: RoadPiece) {
    const v = vertices(piece);
    for (let t = piece.near.first; t < piece.near.first + piece.near.count; t += 3) {
        const ids = [piece.indices[t], piece.indices[t + 1], piece.indices[t + 2]];
        if (!ids.some(id => banded(v.word(id)))) yield { ids, v };
    }
}

/** The faces of `pieceFaces` of each of an owner's L16 pieces. */
function* nearFaces({ pieces }: RoadMesh) {
    for (const piece of pieces) if (piece.level === CHILD) yield* pieceFaces(piece);
}

/** The grid of the face samples, in metres, and in world units at the middle of the fixture, so that every owner has the same grid. */
const FACE_STEP_M = 0.5, FACE_STEP = FACE_STEP_M / metresPerUnit((north + south) / 2);
/** The deck bit of a face sample: it lies within 2 squares of DECK_CELL_M metres of a deck's station. */
export const DECK_BIT = 2 ** 20, DECK_CELL_M = 6;

/**
 * The face samples for the tears and the lifts: per owner, in bounds, at each point of a grid of FACE_STEP_M
 * metres that a face covers in plan, the face's height there, the lowest per point and material. So the
 * samples do not change when the same surface takes other triangles. Faces of a material past RISER
 * (sleepers, rails, walls) stand over the ground by design and give none. Rows of 4 numbers: world x, y,
 * the height, then the material + the half-width's steps × 256 + DECK_BIT about a deck.
 */
export function faceSamples(tiles: FixtureTile[]): Float64Array {
    const decks = new Set<string>(), deckCell = (x: number, y: number) => `${Math.floor(x * metresPerUnit(y) / DECK_CELL_M)},${Math.floor(y * metresPerUnit(y) / DECK_CELL_M)}`;
    for (const { tile, list } of tiles) for (const { line, stations } of planOf(features(list, tile)).runs) if (isDeck(line)) for (const s of stations) {
        const unit = metresPerUnit(s.y);
        for (let a = -2; a <= 2; a++) for (let c = -2; c <= 2; c++) decks.add(`${Math.floor(s.x * unit / DECK_CELL_M) + a},${Math.floor(s.y * unit / DECK_CELL_M) + c}`);
    }
    const rows: number[] = [];
    for (const { meshes } of tiles) for (const mesh of meshes) {
        const lowest = new Map<string, number[]>();
        for (const { ids, v } of nearFaces(mesh)) {
            const word = v.word(ids[0]), mat = word & 127;
            if (mat > LOOK.RISER) continue;
            const [ax, bx, cx] = ids.map(v.x), [ay, by, cy] = ids.map(v.y), [az, bz, cz] = ids.map(v.z), area = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
            // A face that stands upright covers no grid point.
            if (Math.abs(area) < 1e-9) continue;
            for (let i = Math.ceil(Math.min(ax, bx, cx) / FACE_STEP - 0.5); i <= Math.floor(Math.max(ax, bx, cx) / FACE_STEP - 0.5); i++) {
                for (let j = Math.ceil(Math.min(ay, by, cy) / FACE_STEP - 0.5); j <= Math.floor(Math.max(ay, by, cy) / FACE_STEP - 0.5); j++) {
                    const x = (i + 0.5) * FACE_STEP, y = (j + 0.5) * FACE_STEP;
                    const wb = ((x - ax) * (cy - ay) - (cx - ax) * (y - ay)) / area, wc = ((bx - ax) * (y - ay) - (x - ax) * (by - ay)) / area, wa = 1 - wb - wc;
                    if (wa < 0 || wb < 0 || wc < 0 || !inBounds(x, y)) continue;
                    const z = wa * az + wb * bz + wc * cz, name = `${i},${j},${mat}`, had = lowest.get(name);
                    if (!had || z < had[2]) lowest.set(name, [x, y, z, mat + ((word >>> 19) & 511) * 256 + (decks.has(deckCell(x, y)) ? DECK_BIT : 0)]);
                }
            }
        }
        for (const row of lowest.values()) rows.push(...row);
    }
    return new Float64Array(rows);
}

/** The limits of the torn faces. The ends of ways and of sidewalks turn up to about 95° from their neighbours by design. */
const FOLD_DEG = 100, SLIVER_M = 0.02, SLIVER_STEP_M = 0.02, TWIST_DEG = 30;
/** A face under this many square metres has no normal that a fold or a twist can read. */
const LEAST_AREA_M2 = 1e-4;

/** Each kind of torn face: world x, y pairs, worst first: a fold by its turn, a sliver by its height step times its length, a twist by its turn times its area. */
export interface TornFaces { folds: number[]; slivers: number[]; twists: number[] }

/**
 * The torn faces: the near faces of each L16 piece (`pieceFaces`) in the piece's own square and in bounds. A face with
 * a corner of a kerb, a riser or a material past RISER stands upright by design and gives none.
 * - fold: a face whose normal lies more than FOLD_DEG from up, or from the normal of a face of its piece that shares an edge with it.
 * - sliver: a face under SLIVER_M across in plan (twice its plan area over its longest edge in plan) whose corners differ in height by more than SLIVER_STEP_M.
 * - twist: a quad, two faces in a row with the same first vertex and a shared edge, whose normals lie more than TWIST_DEG and up to FOLD_DEG apart. It counts once, at its first face.
 * Two faces share an edge where its ends lie at the same points, also across the edge of the square. The normal faces
 * the side that corridor.wgsl draws. A fold and a twist read no face under LEAST_AREA_M2.
 */
export function tornFaces(pieces: Iterable<RoadPiece>): TornFaces {
    const found = { folds: [] as number[][], slivers: [] as number[][], twists: [] as number[][] }, cos = (deg: number) => Math.cos(deg * Math.PI / 180);
    for (const piece of pieces) {
        if (piece.level !== CHILD) continue;
        const unit = metresPerUnit(piece.y), square = { level: CHILD, x: Math.floor(piece.x / size(CHILD)), y: Math.floor(piece.y / size(CHILD)) };
        const normals: number[] = [], middles: number[] = [], areas: number[] = [], folds: number[] = [], own: boolean[] = [], edges = new Map<number, number[]>();
        const points = new Map<string, number>(), pointOf = new Map<number, number>();
        /** The first vertex at the point of vertex `id`, in steps of x and y and millimetres of z: a seam repeats a point in two vertices. */
        const at = (v: ReturnType<typeof vertices>, id: number) => {
            if (!pointOf.has(id)) {
                const name = `${Math.round((v.x(id) - piece.x) * 12)},${Math.round((v.y(id) - piece.y) * 12)},${Math.round(v.z(id) * 1000)}`;
                pointOf.set(id, points.get(name) ?? points.set(name, id).get(name)!);
            }
            return pointOf.get(id)!;
        };
        let [lead, previous] = [-1, -1];
        for (const { ids, v } of pieceFaces(piece)) {
            const twin = lead === ids[0] ? previous : -1;
            [lead, previous] = [ids[0], -1];
            if (ids.some(id => (v.word(id) & 127) === MAT.KERB || (v.word(id) & 127) >= LOOK.RISER)) continue;
            const [ax, bx, cx] = ids.map(id => (v.x(id) - piece.x) * unit), [ay, by, cy] = ids.map(id => (v.y(id) - piece.y) * unit), [az, bz, cz] = ids.map(v.z);
            const x = piece.x + (ax + bx + cx) / 3 / unit, y = piece.y + (ay + by + cy) / 3 / unit, counts = inSquare(square, x, y);
            // World y runs south: the normal's z is up.
            const nx = (cy - ay) * (bz - az) - (by - ay) * (cz - az), ny = (bx - ax) * (cz - az) - (cx - ax) * (bz - az), nz = (by - ay) * (cx - ax) - (bx - ax) * (cy - ay), area = Math.hypot(nx, ny, nz) / 2;
            const longest = Math.max(Math.hypot(bx - ax, by - ay), Math.hypot(cx - bx, cy - by), Math.hypot(ax - cx, ay - cy)), step = Math.max(az, bz, cz) - Math.min(az, bz, cz);
            if (counts && Math.abs(nz) / longest < SLIVER_M && step > SLIVER_STEP_M) found.slivers.push([step * longest, x, y]);
            if (area < LEAST_AREA_M2) continue;
            const f = (previous = areas.length), up = nz / (2 * area);
            normals.push(nx / (2 * area), ny / (2 * area), up);
            middles.push(x, y);
            areas.push(area);
            own.push(counts);
            folds.push(up < cos(FOLD_DEG) ? Math.acos(up) : 0);
            const corners = ids.map(id => at(v, id));
            for (let k = 0; k < 3; k++) {
                const [p, q] = [corners[k], corners[(k + 1) % 3]];
                if (p === q) continue;
                const key = Math.min(p, q) * 65536 + Math.max(p, q), sharing = edges.get(key) ?? edges.set(key, []).get(key)!;
                for (const other of sharing) {
                    const dot = normals[3 * f] * normals[3 * other] + normals[3 * f + 1] * normals[3 * other + 1] + normals[3 * f + 2] * normals[3 * other + 2], turn = Math.acos(Math.max(-1, Math.min(1, dot)));
                    if (dot < cos(FOLD_DEG)) for (const g of [f, other]) folds[g] = Math.max(folds[g], turn);
                    if (other === twin && own[twin] && dot < cos(TWIST_DEG) && dot >= cos(FOLD_DEG)) found.twists.push([turn * (area + areas[twin]), middles[2 * twin], middles[2 * twin + 1]]);
                }
                sharing.push(f);
            }
        }
        folds.forEach((turn, f) => {
            if (turn && own[f]) found.folds.push([turn, middles[2 * f], middles[2 * f + 1]]);
        });
    }
    const worstFirst = (rows: number[][]) => rows.sort((a, b) => b[0] - a[0]).flatMap(([, x, y]) => [x, y]);
    return { folds: worstFirst(found.folds), slivers: worstFirst(found.slivers), twists: worstFirst(found.twists) };
}

/** The step of the gap samples, in metres. */
const GAP_STEP_M = 0.25;
/** World units per side of a cell of the gap test's index of faces, about 6.5 m. */
const INDEX_UNITS = 256;

/** Whether a world point lies in plan in a near face of any owner of the tiles. */
function coveredBy(tiles: FixtureTile[]): (x: number, y: number) => boolean {
    // About an origin west and north of the tiles, so that float32 holds the faces to a millimetre.
    const x0 = Math.min(...tiles.map(({ tile }) => tile.x - 1)) * size(DEEPEST), y0 = Math.min(...tiles.map(({ tile }) => tile.y - 1)) * size(DEEPEST);
    let n = 0;
    for (const { meshes } of tiles) for (const mesh of meshes) for (const _ of nearFaces(mesh)) n++;
    const plan = new Float32Array(6 * n), cells = new Map<number, number[]>(), key = (i: number, j: number) => i * 65536 + j;
    let f = 0;
    for (const { meshes } of tiles) for (const mesh of meshes) for (const { ids, v } of nearFaces(mesh)) {
        let [x1, y1, x2, y2] = [Infinity, Infinity, -Infinity, -Infinity];
        ids.forEach((id, k) => {
            const [x, y] = [v.x(id) - x0, v.y(id) - y0];
            plan.set([x, y], 6 * f + 2 * k);
            [x1, y1, x2, y2] = [Math.min(x1, x), Math.min(y1, y), Math.max(x2, x), Math.max(y2, y)];
        });
        for (let i = Math.floor(x1 / INDEX_UNITS); i <= Math.floor(x2 / INDEX_UNITS); i++) {
            for (let j = Math.floor(y1 / INDEX_UNITS); j <= Math.floor(y2 / INDEX_UNITS); j++) (cells.get(key(i, j)) ?? cells.set(key(i, j), []).get(key(i, j))!).push(f);
        }
        f++;
    }
    /** Whether a point lies in face f: on the same side of its three edges, or on one. */
    const inside = (f: number, x: number, y: number) => {
        const o = 6 * f, [ax, ay, bx, by, cx, cy] = [plan[o], plan[o + 1], plan[o + 2], plan[o + 3], plan[o + 4], plan[o + 5]];
        const d0 = (x - bx) * (ay - by) - (ax - bx) * (y - by), d1 = (x - cx) * (by - cy) - (bx - cx) * (y - cy), d2 = (x - ax) * (cy - ay) - (cx - ax) * (y - ay);
        return (d0 <= 0 && d1 <= 0 && d2 <= 0) || (d0 >= 0 && d1 >= 0 && d2 >= 0);
    };
    // The next point across a way most often lies in the face that held the last one.
    let last = -1;
    return (x, y) => {
        const [px, py] = [x - x0, y - y0];
        if (last >= 0 && inside(last, px, py)) return true;
        for (const f of cells.get(key(Math.floor(px / INDEX_UNITS), Math.floor(py / INDEX_UNITS))) ?? []) {
            if (inside(f, px, py)) {
                last = f;
                return true;
            }
        }
        return false;
    };
}

/**
 * The gaps: points of a way's core that no face draws. Samples lie about GAP_STEP_M metres apart along and
 * across each step between two stations of each way that has a corridor, from the core's edge on its right to
 * its edge on its left at each foot (`stepSide`), half a step inside them, in the tile's own square. A sample is a gap
 * where no near face of any owner's corridor covers it in plan; a band's faces cover nothing. Returns world x, y pairs.
 */
export function gaps(tiles: FixtureTile[]): number[] {
    const covered = coveredBy(tiles), out: number[] = [];
    for (const { tile, list } of tiles) for (const { line, stations: st } of planOf(features(list, tile)).runs) {
        for (let k = 0; k + 1 < st.length; k++) {
            const s = st[k], along = Math.max(1, Math.round(Math.hypot(st[k + 1].x - s.x, st[k + 1].y - s.y) * metresPerUnit(s.y) / GAP_STEP_M));
            for (let n = 0; n < along; n++) {
                const foot = between(s, st[k + 1], (n + 0.5) / along), left = stepSide(line, st[k + 1], foot, 1).edge, right = stepSide(line, st[k + 1], foot, -1).edge;
                const across = Math.max(1, Math.round((left + right) / GAP_STEP_M));
                for (let i = 0; i < across; i++) {
                    const a = -right + (i + 0.5) * (left + right) / across, x = foot.x + foot.ax * a, y = foot.y + foot.ay * a;
                    if (inSquare(tile, x, y) && !covered(x, y)) out.push(x, y);
                }
            }
        }
    }
    return out;
}

/** A builder that keeps nothing: a point kind's meshes do not count. */
const nothing = () => ({ mesh: new MeshBuilder(), anchored: new AnchoredBuilder([]) });

/**
 * The objects on ways: each model that a kind places in the tile's own square, as the stamp worker builds every
 * feature, whose foot lies in the core of a way (`coreAt` of roads/cores.ts) or in a junction's corner. Street lamps and culvert
 * mouths stand in a core by design: the worker places them apart from the kinds, and they do not count. Returns
 * world x, y pairs.
 */
export function objectsOnWays(tiles: FixtureTile[]): number[] {
    const out: number[] = [];
    for (const { tile, list } of tiles) {
        const shapes = features(list, tile), { lines, buildings } = shapes, cores = coresOf(shapes), cornered = (x: number, y: number) => !!cornerAt(lines, x, y);
        const build: Build = {
            far: nothing(), near: nothing(), owns: (x, y) => inSquare(tile, x, y), ways: lines.filter(line => line.kind === ROAD || line.kind === PATH), cores, taken: [], buildings, cornered,
            place: ({ x, y }) => {
                if (coreAt(cores, x, y) || cornered(x, y)) out.push(x, y);
            },
        };
        for (const feature of list) KINDS.get(feature.kind)?.build(placed(feature, tile), build);
    }
    return out;
}

/** The bake leaves no sidewalk shorter than this. */
const SIDEWALK_LEAST_M = 50;

/** A segment about a vertex, pointing away from it: its angle counter-clockwise from east, and the sidewalks on its away-left and its away-right, -1 for none. */
export interface WayOut { angle: number; left: number; right: number }

/** Whether a line's ways out stand between the corners of a vertex: a class with a sidewalk width, as `can` of `add_ways_out` in the bake. */
export const splitsCorners = (line: Line) => walkOf(line) > 0;

/**
 * The sidewalks that turn a corner about a vertex, as the bake joins them (`drop_short` of
 * host/obc-trailview-features/src/roads/sidewalks.rs): the sidewalk right of one way out runs on left of the
 * next clockwise. Each way out of a class with a sidewalk width stands between them, also where it has none.
 */
export function corners(around: WayOut[]): [number, number][] {
    if (around.length < 2) return [];
    const sorted = [...around].sort((a, b) => b.angle - a.angle);
    return sorted.map((way, i) => [way.right, sorted[(i + 1) % sorted.length].left]);
}

/**
 * The short sidewalks. A sidewalk is one connected run on one side of the ways: it runs on across a vertex
 * into the sidewalk on the same side of the next way about that vertex, as it turns a junction's corner, and
 * its length is the sum over all its segments. One under SIDEWALK_LEAST_M counts, at the middle of its first
 * segment by position, in the tile whose own square holds that point. One that stops at a way's end in the
 * tile's border does not count: the tile may cut the way there. Returns world x, y pairs.
 */
export function shortSidewalks(tiles: FixtureTile[]): number[] {
    const out: number[] = [];
    for (const { tile, list } of tiles) {
        const lines = features(list, tile).lines.filter(splitsCorners);
        // A node per segment and side, LEFT or RIGHT; the sides of one sidewalk share a root.
        const nodes: { line: Line; v: number }[] = [], ids = new Map<Line, number>(), parent: number[] = [];
        for (const line of lines) {
            ids.set(line, nodes.length);
            for (let v = 0; v + 1 < line.z.length; v++) for (const _ of [LEFT, RIGHT]) parent.push(nodes.push({ line, v }) - 1);
        }
        const node = (line: Line, v: number, bit: number) => ((line.sidewalks?.[v] ?? 0) & bit ? ids.get(line)! + 2 * v + (bit === LEFT ? 0 : 1) : -1);
        const root = (n: number): number => (parent[n] === n ? n : (parent[n] = root(parent[n])));
        const join = (a: number, b: number) => a >= 0 && b >= 0 && (parent[root(a)] = root(b));
        // Per vertex, each segment with a sidewalk that leaves it, pointing away: its away-left and away-right sidewalk.
        const ends = new Map<string, (WayOut & { end: boolean })[]>();
        for (const line of lines) for (let v = 0; v + 1 < line.z.length; v++) {
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4);
            // World y runs south: an angle counter-clockwise from east.
            (ends.get(`${ax},${ay}`) ?? ends.set(`${ax},${ay}`, []).get(`${ax},${ay}`)!).push({ angle: Math.atan2(ay - by, bx - ax), left: node(line, v, LEFT), right: node(line, v, RIGHT), end: v === 0 });
            (ends.get(`${bx},${by}`) ?? ends.set(`${bx},${by}`, []).get(`${bx},${by}`)!).push({ angle: Math.atan2(by - ay, ax - bx), left: node(line, v, RIGHT), right: node(line, v, LEFT), end: v === line.z.length - 2 });
        }
        // A dead end has no next segment.
        const open = new Set<number>();
        for (const [name, around] of ends) {
            if (around.length === 1) {
                const [x, y] = name.split(',').map(Number);
                if (around[0].end && !inSquare(tile, x, y)) for (const n of [around[0].left, around[0].right]) if (n >= 0) open.add(n);
                continue;
            }
            for (const [a, b] of corners(around)) join(a, b);
        }
        const runs = new Map<number, { metres: number; first: number[]; open: boolean }>();
        nodes.forEach(({ line, v }, n) => {
            if (node(line, v, n % 2 === 0 ? LEFT : RIGHT) !== n) return;
            const [ax, ay, bx, by] = line.xy.slice(2 * v, 2 * v + 4), run = runs.get(root(n)) ?? runs.set(root(n), { metres: 0, first: [Infinity, Infinity], open: false }).get(root(n))!;
            run.metres += Math.hypot(bx - ax, by - ay) * metresPerUnit(ay);
            run.open ||= open.has(n);
            const middle = [(ax + bx) / 2, (ay + by) / 2];
            if (middle[0] < run.first[0] || (middle[0] === run.first[0] && middle[1] < run.first[1])) run.first = middle;
        });
        for (const { metres, first: [x, y], open: cut } of runs.values()) if (metres < SIDEWALK_LEAST_M && !cut && inSquare(tile, x, y)) out.push(x, y);
    }
    return out;
}

/** The grid of the water samples, in metres. */
const WATER_STEP_M = 0.5;

/**
 * The water samples for plants in water: the points of a grid of WATER_STEP_M metres over the tile's own
 * square, in bounds, that lie in a stream's or a river's ribbon (its half-width about its line), a lake or a
 * river area. Rows of 3 numbers: world x, y and the water's height, NaN in a river area, which lies on the
 * drawn ground.
 */
export function waterSamples(tiles: FixtureTile[]): Float64Array {
    const rows: number[] = [];
    for (const { tile, list } of tiles) {
        const side = size(tile.level), x0 = tile.x * side, y0 = tile.y * side, step = WATER_STEP_M / metresPerUnit(y0 + side / 2), cells = Math.ceil(side / step), seen = new Set<number>();
        const range = (lo: number, hi: number, o: number) => [Math.max(0, Math.ceil((lo - o) / step - 0.5)), Math.min(cells - 1, Math.floor((hi - o) / step - 0.5))];
        /** Each grid point in a box once, with the height that `z` gives it, which is undefined out of the water. */
        const scan = (xs: number[], ys: number[], z: (x: number, y: number) => number | undefined) => {
            const [i0, i1] = range(Math.min(...xs), Math.max(...xs), x0), [j0, j1] = range(Math.min(...ys), Math.max(...ys), y0);
            for (let i = i0; i <= i1; i++) for (let j = j0; j <= j1; j++) {
                if (seen.has(i * cells + j)) continue;
                const x = x0 + (i + 0.5) * step, y = y0 + (j + 0.5) * step, height = inBounds(x, y) ? z(x, y) : undefined;
                if (height === undefined) continue;
                seen.add(i * cells + j);
                rows.push(x, y, height);
            }
        };
        const { lines, lakes, areas } = features(list, tile);
        for (const { kind, xy, z, h } of lines) {
            if (kind !== STREAM && kind !== RIVER) continue;
            for (let v = 0; v + 1 < z.length; v++) {
                const [ax, ay, bx, by] = xy.slice(2 * v, 2 * v + 4), reach = h / metresPerUnit(ay), dx = bx - ax, dy = by - ay;
                scan([ax - reach, ax + reach, bx - reach, bx + reach], [ay - reach, ay + reach, by - reach, by + reach], (x, y) => {
                    const u = Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy || 1)));
                    return Math.hypot(x - ax - u * dx, y - ay - u * dy) <= reach ? z[v] + (z[v + 1] - z[v]) * u : undefined;
                });
            }
        }
        for (const { rings, level } of [...lakes, ...areas.map(area => ({ ...area, level: NaN }))]) {
            const all = rings.flat();
            scan(all.filter((_, i) => i % 2 === 0), all.filter((_, i) => i % 2 === 1), (x, y) => (insideRings(rings, x, y) ? level : undefined));
        }
    }
    return new Float64Array(rows);
}
