// The fault census of the TrailView fixture, on the CPU: the counts that the feature tiles and the corridor
// meshes decide, and the samples at which census.mjs reads the drawn terrain and the mask on the GPU.
// Each count's doc comment is its definition; the README's "Shots and flights" says the same in short.
import { readdirSync, readFileSync } from 'node:fs';
import { gunzipSync } from 'node:zlib';
import config from '../../tests/browser/trailview/fixture/config.json' with { type: 'json' };
import { worldPixel } from '../../src/lib/planner/layers/mercator';
import { UNITS } from '../../src/lib/trailview/frame/space';
import { isDeck } from '../../src/lib/trailview/roads/decks';
import { planOf, roadMesh, type RoadMesh } from '../../src/lib/trailview/roads/mesh';
import type { RoadPiece } from '../../src/lib/trailview/roads/piece';
import { CF, corridorLine, LOOK, sideOf } from '../../src/lib/trailview/roads/section';
import { decodeFeatures, DEEPEST, EXTENT, unitScale, type Feature } from '../../src/lib/trailview/stream/features';
import { features, metresPerUnit, RIVER, STREAM, type Line } from '../../src/lib/trailview/terrain/bins';
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

/** The census line of a count: its samples (world x, y pairs) and its places. */
export function countLine(count: string, points: ArrayLike<number>): string {
    const squares = new Map<string, number>();
    for (let i = 0; i < points.length; i += 2) {
        const unit = metresPerUnit(points[i + 1]), name = `${Math.floor(points[i] * unit / PLACE_M)},${Math.floor(points[i + 1] * unit / PLACE_M)}`;
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

/** A piece's vertices as the census reads them: world x, y, z and the look word. */
function vertices({ x, y, vertices }: RoadPiece) {
    const steps = new Int16Array(vertices), words = new Uint32Array(vertices), floats = new Float32Array(vertices);
    return {
        x: (v: number) => x + (steps[8 * v] * 4 + ((words[4 * v + 2] >>> 28) & 3)) / 12,
        y: (v: number) => y + (steps[8 * v + 1] * 4 + (words[4 * v + 2] >>> 30)) / 12,
        z: (v: number) => floats[4 * v + 1],
        word: (v: number) => words[4 * v + 2],
    };
}

/** Whether a vertex is a band's row, whose height exists only on the GPU. */
const banded = (word: number) => (((word >>> 11) & 31) & (CF.SAMPLE | CF.MESH)) !== 0;

/** Each drawn face of the near range of an owner's L16 pieces that has no band row: the faces of a way's footprint. */
function* nearFaces({ pieces }: RoadMesh) {
    for (const piece of pieces) {
        if (piece.level !== CHILD) continue;
        const v = vertices(piece);
        for (let t = piece.near.first; t < piece.near.first + piece.near.count; t += 3) {
            const ids = [piece.indices[t], piece.indices[t + 1], piece.indices[t + 2]];
            if (!ids.some(id => banded(v.word(id)))) yield { ids, v };
        }
    }
}

/** The grid of the face samples, in metres. */
const FACE_STEP_M = 0.5;
/** The deck bit of a face sample: it lies within 2 squares of DECK_CELL_M metres of a deck's station. */
export const DECK_BIT = 2 ** 20, DECK_CELL_M = 6;

/**
 * The face samples for the tears and the lifts: per owner, in bounds, on a grid of FACE_STEP_M metres per material, the
 * lowest of each face's corners and middle. Faces of a material past RISER (sleepers, rails, walls) stand over
 * the ground by design and give none. Rows of 4 numbers: world x, y, the face's height, then its material +
 * its half-width's steps × 256 + DECK_BIT about a deck.
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
            const corners = ids.map(id => [v.x(id), v.y(id), v.z(id)]), middle = [0, 1, 2].map(k => (corners[0][k] + corners[1][k] + corners[2][k]) / 3);
            for (const [x, y, z] of [middle, ...corners]) {
                if (!inBounds(x, y)) continue;
                const unit = metresPerUnit(y), name = `${Math.round(x * unit / FACE_STEP_M)},${Math.round(y * unit / FACE_STEP_M)},${mat}`, had = lowest.get(name);
                if (!had || z < had[2]) lowest.set(name, [x, y, z, mat + ((word >>> 19) & 511) * 256 + (decks.has(deckCell(x, y)) ? DECK_BIT : 0)]);
            }
        }
        for (const row of lowest.values()) rows.push(...row);
    }
    return new Float64Array(rows);
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
 * The gaps: points of a way's carriageway that no face draws. Samples lie about GAP_STEP_M metres apart along
 * and across the carriageway of each way that has a corridor, half a step inside its edges, between each two
 * of its stations, in the tile's own square. A sample is a gap where no near face of any owner's corridor
 * covers it in plan; a band's faces cover nothing. Returns world x, y pairs.
 */
export function gaps(tiles: FixtureTile[]): number[] {
    const covered = coveredBy(tiles), out: number[] = [];
    for (const { tile, list } of tiles) for (const { stations } of planOf(features(list, tile)).runs) {
        for (let j = 0; j + 1 < stations.length; j++) {
            const s = stations[j], t = stations[j + 1], along = Math.max(1, Math.round(Math.hypot(t.x - s.x, t.y - s.y) * metresPerUnit(s.y) / GAP_STEP_M));
            for (let k = 0; k < along; k++) {
                const u = (k + 0.5) / along, mix = (a: number, b: number) => a + (b - a) * u, h = mix(s.h, t.h), across = Math.max(1, Math.round(2 * h / GAP_STEP_M));
                const [x, y, ax, ay] = [mix(s.x, t.x), mix(s.y, t.y), mix(s.ax, t.ax), mix(s.ay, t.ay)];
                for (let i = 0; i < across; i++) {
                    const a = -h + (i + 0.5) * 2 * h / across, px = x + ax * a, py = y + ay * a;
                    if (inSquare(tile, px, py) && !covered(px, py)) out.push(px, py);
                }
            }
        }
    }
    return out;
}

/** The kinds of objects that must stand beside a way. The map has no kind for a bin. */
const OBJECTS = ['guidepost', 'bench', 'picnic-table'];

/** Whether a world point lies in the core of a line: within its half-width and its sidewalk or its shoulder on that side of it. */
function inCore(line: Line, x: number, y: number): boolean {
    const { xy } = line;
    for (let v = 0; v + 2 < xy.length; v += 2) {
        const dx = xy[v + 2] - xy[v], dy = xy[v + 3] - xy[v + 1], px = x - xy[v], py = y - xy[v + 1];
        const u = Math.max(0, Math.min(1, (px * dx + py * dy) / (dx * dx + dy * dy || 1))), metres = Math.hypot(px - u * dx, py - u * dy) * metresPerUnit(y);
        if (metres <= sideOf(line, px * dy - py * dx > 0 ? 1 : -1, v / 2, line.h).edge) return true;
    }
    return false;
}

/**
 * The objects on ways: each point of a guidepost, a bench or a picnic table in the tile's own square that lies
 * in the core of a way that has a corridor. Returns world x, y pairs.
 */
export function objectsOnWays(tiles: FixtureTile[]): number[] {
    const out: number[] = [];
    for (const { tile, list } of tiles) {
        const scale = unitScale(tile.level), lines = features(list, tile).lines.filter(corridorLine);
        for (const { kind, geometry, parts } of list) {
            if (geometry !== 'point' || !OBJECTS.includes(kind)) continue;
            for (const { xy } of parts) for (let i = 0; i < xy.length; i += 2) {
                const x = (tile.x * EXTENT + xy[i]) * scale, y = (tile.y * EXTENT + xy[i + 1]) * scale;
                if (inSquare(tile, x, y) && lines.some(line => inCore(line, x, y))) out.push(x, y);
            }
        }
    }
    return out;
}

/** The bake leaves no sidewalk piece shorter than this. */
const SIDEWALK_LEAST_M = 50;

/**
 * The short sidewalks: per side of each line, each run of its segments with a sidewalk on that side that is
 * shorter than SIDEWALK_LEAST_M metres and stops at both ends, at the middle of the run, in the tile whose
 * own square holds that middle. A run does not stop at a line's end where another line with a sidewalk
 * meets it: the sidewalk runs on there. A run that ends at a line's end in the tile's border does not
 * count: the tile may cut the line there. Returns world x, y pairs.
 */
export function shortSidewalks(tiles: FixtureTile[]): number[] {
    const out: number[] = [];
    for (const { tile, list } of tiles) {
        const lines = features(list, tile).lines, walked = new Map<string, Set<Line>>(), at = ({ xy }: Line, v: number) => `${xy[2 * v]},${xy[2 * v + 1]}`;
        for (const line of lines) line.sidewalks?.forEach((sides, v) => {
            if (sides) for (const end of [v, v + 1]) (walked.get(at(line, end)) ?? walked.set(at(line, end), new Set()).get(at(line, end))!).add(line);
        });
        for (const line of lines) {
            const { xy, sidewalks } = line;
            if (!sidewalks) continue;
            const last = sidewalks.length, metres = (v: number) => Math.hypot(xy[2 * v + 2] - xy[2 * v], xy[2 * v + 3] - xy[2 * v + 1]) * metresPerUnit(xy[2 * v + 1]);
            /** Whether a run stops at vertex v: inside the line it does; at the line's end, where no other line's sidewalk meets it, in the tile's own square. */
            const stops = (v: number) => (v > 0 && v < last) || (walked.get(at(line, v))!.size === 1 && inSquare(tile, xy[2 * v], xy[2 * v + 1]));
            for (const bit of [LEFT, RIGHT]) {
                for (let v = 0; v < last; v++) {
                    if (!(sidewalks[v] & bit)) continue;
                    let end = v;
                    while (end < last && sidewalks[end] & bit) end++;
                    const lengths = Array.from({ length: end - v }, (_, k) => metres(v + k)), length = lengths.reduce((a, b) => a + b, 0);
                    if (length < SIDEWALK_LEAST_M && stops(v) && stops(end)) {
                        let w = 0, rest = length / 2;
                        while (w + 1 < lengths.length && rest > lengths[w]) rest -= lengths[w++];
                        const u = rest / lengths[w] || 0, s = v + w, x = xy[2 * s] + (xy[2 * s + 2] - xy[2 * s]) * u, y = xy[2 * s + 1] + (xy[2 * s + 3] - xy[2 * s + 1]) * u;
                        if (inSquare(tile, x, y)) out.push(x, y);
                    }
                    v = end;
                }
            }
        }
    }
    return out;
}

/** The grid of the water samples, in metres. */
const WATER_STEP_M = 0.5;

/** Whether a point lies inside rings by the even-odd rule, so that a hole is outside. */
function insideRings(rings: number[][], x: number, y: number): boolean {
    let inside = false;
    for (const ring of rings) for (let i = 0, j = ring.length - 2; i < ring.length; j = i, i += 2) {
        if (ring[i + 1] > y !== ring[j + 1] > y && x < ring[j] + (y - ring[j + 1]) * (ring[i] - ring[j]) / (ring[i + 1] - ring[j + 1])) inside = !inside;
    }
    return inside;
}

/**
 * The water samples for plants in water: the points of a grid of WATER_STEP_M metres over the tile's own
 * square, in bounds, that lie in a stream's or a river's ribbon (its half-width about its line), a lake or a river area.
 * Rows of 3 numbers: world x, y and the water's height, NaN in a river area, which lies on the drawn ground.
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
