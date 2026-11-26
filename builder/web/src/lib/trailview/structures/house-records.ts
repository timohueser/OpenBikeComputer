// The houses of a feature tile as houses.wgsl reads them: a record per building that the tile owns,
// and a mesh of walls and a roof for the footprints that no template fits. A worker uses it.
import earcut from 'earcut';
import { EXTENT, square, unitScale, type Feature } from '../stream/features';
import { fronts, metresPerUnit, type Node } from '../terrain/bins';

/** Words of a record, as `House` in houses.wgsl. */
export const RECORD_WORDS = 8;
/** Words of an outline vertex, as `OutlineIn` in houses.wgsl. */
export const OUTLINE_WORDS = 4;

/** The kinds of house, as `KINDS` in houses.wgsl. */
export const COTTAGE = 0, HOUSE = 1, FARMHOUSE = 2, BARN = 3, BLOCK = 4, CHURCH = 5, INDUSTRIAL = 6, GRANARY = 7, GREENHOUSE = 8;
/** Shapes: a template of models/house.ts by its kind, or an outline mesh. */
export const TEMPLATE = 0, OUTLINE = 1;
/** Roofs that OSM names, as `shape_of` in houses.wgsl reads them: the style's, or a gable, a hip or a half hip. A flat roof is a flag. */
const ROOFS: Record<string, number> = { gabled: 1, hipped: 2, pyramidal: 2, 'half-hipped': 3 };

/** OSM building values that draw nothing, draw as a barn or shed, as a church or as a block, from the prototype's nearfield.ts. */
const SKIPPED = new Set(['roof', 'ruins', 'construction', 'carport', 'storage_tank', 'bunker']);
const SHEDS = new Set(['barn', 'farm_auxiliary', 'shed', 'garage', 'garages', 'stable', 'cowshed', 'hut', 'sty', 'boathouse']);
/** Granaries this small stand raised on pillars, as hórreos and stabbur do; larger ones are barns. */
const GRANARY_M2 = 60;
/** A greenhouse's walls in metres, as its kind in houses.wgsl. */
const GREENHOUSE_WALL_M = 3.5;
const CHURCHES = new Set(['church', 'chapel', 'cathedral']);
const BLOCKS = new Set([
    'apartments', 'residential', 'hotel', 'commercial', 'retail', 'office', 'industrial', 'warehouse', 'school', 'public', 'civic',
    'hospital', 'university', 'college', 'terrace', 'dormitory', 'train_station', 'supermarket', 'kindergarten', 'sports_hall',
    'government', 'manufacture', 'service',
]);
/** Blocks of these kinds are one tall storey or two, not flats. */
const LOW = new Set(['industrial', 'warehouse', 'supermarket', 'retail', 'manufacture', 'sports_hall', 'service']);
/** Halls of the same look in every region; so is a `yes` building in an industrial area. */
const INDUSTRIES = new Set(['industrial', 'warehouse', 'manufacture']);
const STOREY_M = 2.8;
/**
 * A footprint that fills this much of its rectangle draws as that rectangle. A footprint that fills
 * less draws as at most PARTS rectangles where its sides meet at right angles (an L, a T, a U), and
 * else along its outline.
 */
const RECTANGULAR = 0.7;
const PARTS = 4;
/** Sides of a footprint within this many degrees of its rectangle's sides, and corners closer than SNAP_M, count as square. */
const SQUARE_DEG = 12, SNAP_M = 0.5;
/**
 * An outline moved inward by `d` metres, or outward by a negative `d`, each corner along its bisector; null where a
 * side would turn round or two sides would cross. The outline is counter-clockwise.
 */
function inset(ring: number[][], d: number): number[][] | null {
    const n = ring.length, out: number[][] = [];
    for (let i = 0; i < n; i++) {
        const a = ring[(i + n - 1) % n], p = ring[i], b = ring[(i + 1) % n], l0 = Math.hypot(p[0] - a[0], p[1] - a[1]), l1 = Math.hypot(b[0] - p[0], b[1] - p[1]);
        const n0 = [(a[1] - p[1]) / l0, (p[0] - a[0]) / l0], n1 = [(p[1] - b[1]) / l1, (b[0] - p[0]) / l1], k = 1 + n0[0] * n1[0] + n0[1] * n1[1];
        // A spike has no bisector to move along.
        if (!(k > 0.1)) return null;
        out.push([p[0] + d * (n0[0] + n1[0]) / k, p[1] + d * (n0[1] + n1[1]) / k]);
    }
    const side = (i: number) => [out[i], out[(i + 1) % n]], turn = (a: number[], b: number[], c: number[]) => (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    for (let i = 0; i < n; i++) {
        const [a, b] = side(i), p = ring[i], q = ring[(i + 1) % n];
        if ((b[0] - a[0]) * (q[0] - p[0]) + (b[1] - a[1]) * (q[1] - p[1]) <= 0) return null;
        for (let j = i + 2; j < n; j++) {
            if (i === 0 && j === n - 1) continue;
            const [c, e] = side(j);
            if (turn(a, b, c) * turn(a, b, e) < 0 && turn(c, e, a) * turn(c, e, b) < 0) return null;
        }
    }
    return out;
}

/**
 * An outline's roof stands this far past its walls and has at most this many bands. An outline of more corners
 * than ROOF_CORNERS keeps a flat roof: its slopes would take more vertices than a town can hold. Corners closer
 * than MET_M have met.
 */
const OVERHANG_M = 0.4, ROOF_BANDS = 8, ROOF_CORNERS = 12, MET_M = 0.02;

/** A band of an outline's roof: from one outline, `from` metres inward of the walls, to the next, `to` metres inward. */
interface Band { low: number[][]; high: number[][]; from: number; to: number }

/**
 * The roof of an outline, hipped: the outline moves inward and rises; where a side closes, its two corners become one
 * and the rest moves on, until nothing is left (a ridge or a point) or until two sides meet that are no neighbours.
 * What is left then is the flat `top`. `eaves` stand `over` metres past the `walls`. Null for an outline that cannot
 * move inward at all or has more than ROOF_CORNERS corners.
 */
function hipped(outline: number[][], reach: number): { walls: number[][]; eaves: number[][]; over: number; bands: Band[]; top: number[][]; run: number } | null {
    const apart = (ring: number[][], least: number) => ring.filter((p, i) => Math.hypot(p[0] - ring[(i + 1) % ring.length][0], p[1] - ring[(i + 1) % ring.length][1]) > least);
    // Without the corners that repeat: a closed ring names its first corner twice.
    const walls = apart(outline, 0.01), bands: Band[] = [];
    if (walls.length > ROOF_CORNERS) return null;
    let ring = walls, run = 0;
    while (ring.length >= 3 && bands.length < ROOF_BANDS && run < reach) {
        let low = 0, high = reach - run;
        for (let step = 0; step < 14; step++) {
            const middle = (low + high) / 2;
            if (inset(ring, middle)) low = middle;
            else high = middle;
        }
        const next = low > 0.02 ? inset(ring, low) : null;
        if (!next) break;
        bands.push({ low: ring, high: next, from: run, to: run + low });
        run += low;
        // The corners that met: the side between them closed.
        const left = apart(next, MET_M);
        ring = left;
        if (left.length === next.length) break;
    }
    if (!bands.length) return null;
    const eaves = inset(walls, -OVERHANG_M);
    return { walls, eaves: eaves ?? walls, over: eaves ? OVERHANG_M : 0, bands, top: ring.length >= 3 ? ring : [], run };
}

/** A part narrower than NARROW_M that covers less than MISSED of the footprint goes; the parts may miss MISSED of it. */
const NARROW_M = 2.5, MISSED = 0.08;
/**
 * Part k is k × INSET_M smaller on each side, so that the walls of overlapping parts never share a
 * plane: more than the rounding of two parts' middles to world units moves them apart.
 */
const INSET_M = 0.04;
/** The wall of an outline without levels or a height. */
const OUTLINE_WALL_M = 8;

export interface TileHouses {
    records: Uint32Array<ArrayBuffer>;
    outlines: Uint32Array<ArrayBuffer>;
}

/** The kind of a building by its tag, its area in m² and the landuse that the bake gives it. */
function kindOf(tag: string, area: number, landuse?: string): number {
    if (tag === 'granary' || tag === 'horreo') return area <= GRANARY_M2 ? GRANARY : BARN;
    if (tag === 'greenhouse') return GREENHOUSE;
    if (SHEDS.has(tag)) return BARN;
    if (CHURCHES.has(tag)) return CHURCH;
    if (tag === 'farm') return FARMHOUSE;
    if (INDUSTRIES.has(tag)) return INDUSTRIAL;
    if (BLOCKS.has(tag)) return area < 260 ? HOUSE : BLOCK;
    if (area < 30) return BARN;
    if (landuse === 'industrial' && tag === 'yes') return INDUSTRIAL;
    // A farmyard has one house or two of a farm's size; the buildings smaller or larger are its sheds and barns.
    if (landuse === 'farmyard' && tag === 'yes') return area >= 60 && area < 250 ? FARMHOUSE : BARN;
    if (area < 110) return COTTAGE;
    if (area < 260) return HOUSE;
    if (area < 700) return FARMHOUSE;
    return BLOCK;
}

/** The smallest rectangle about an outline with one side along one of its edges: its middle, the angle of its first side, and its sides. */
function rectangle(ring: number[][]): { x: number; y: number; angle: number; width: number; depth: number } {
    let best = { area: Infinity, x: 0, y: 0, angle: 0, width: 0, depth: 0 };
    ring.forEach((a, i) => {
        const b = ring[(i + 1) % ring.length], angle = Math.atan2(b[1] - a[1], b[0] - a[0]), c = Math.cos(angle), s = Math.sin(angle);
        let [u0, u1, v0, v1] = [Infinity, -Infinity, Infinity, -Infinity];
        for (const [x, y] of ring) {
            const u = x * c + y * s, v = -x * s + y * c;
            [u0, u1, v0, v1] = [Math.min(u0, u), Math.max(u1, u), Math.min(v0, v), Math.max(v1, v)];
        }
        const area = (u1 - u0) * (v1 - v0);
        if (area >= best.area) return;
        const u = (u0 + u1) / 2, v = (v0 + v1) / 2;
        best = { area, x: u * c - v * s, y: u * s + v * c, angle, width: u1 - u0, depth: v1 - v0 };
    });
    return best;
}

const twiceArea = (ring: number[][]) => ring.reduce((sum, [x, y], i) => {
    const [px, py] = ring[(i + ring.length - 1) % ring.length];
    return sum + px * y - x * py;
}, 0);

interface Part { x: number; y: number; width: number; depth: number }

/**
 * The rectangles that cover a footprint whose sides meet at right angles, in metres about the
 * footprint's rectangle and along its sides; null for any other footprint. Each is as large as the
 * footprint allows, so the parts overlap where they meet and their roofs cross.
 */
function parts(ring: number[][], fit: { x: number; y: number; angle: number }): Part[] | null {
    const c = Math.cos(fit.angle), s = Math.sin(fit.angle);
    const local = ring.map(([x, y]) => [(x - fit.x) * c + (y - fit.y) * s, -(x - fit.x) * s + (y - fit.y) * c]);
    for (let i = 0; i < local.length; i++) {
        const [ax, ay] = local[i], [bx, by] = local[(i + 1) % local.length], length = Math.hypot(bx - ax, by - ay);
        const off = Math.abs(Math.atan2(by - ay, bx - ax) * 180 / Math.PI) % 90;
        if (length > 2 * SNAP_M && Math.min(off, 90 - off) > SQUARE_DEG) return null;
    }
    const lines = (k: number) => local.map(p => p[k]).sort((a, b) => a - b).filter((v, i, all) => i === 0 || v - all[i - 1] > SNAP_M);
    const xs = lines(0), ys = lines(1);
    if (xs.length > 12 || ys.length > 12) return null;
    const inside = (px: number, py: number) => local.reduce((odd, [ax, ay], i) => {
        const [bx, by] = local[(i + 1) % local.length];
        return ay > py !== by > py && px < ax + (py - ay) * (bx - ax) / (by - ay) ? !odd : odd;
    }, false);
    const cells = xs.slice(1).map((x, i) => ys.slice(1).map((y, j) => inside((xs[i] + x) / 2, (ys[j] + y) / 2)));
    const area = (i0: number, i1: number, j0: number, j1: number) => (xs[i1 + 1] - xs[i0]) * (ys[j1 + 1] - ys[j0]);
    // The rectangles of cells inside that no larger one holds.
    const full = (i0: number, i1: number, j0: number, j1: number) => {
        for (let i = i0; i <= i1; i++) for (let j = j0; j <= j1; j++) if (!cells[i][j]) return false;
        return true;
    };
    const boxes: number[][] = [];
    for (let i0 = 0; i0 < cells.length; i0++) for (let i1 = i0; i1 < cells.length; i1++)
        for (let j0 = 0; j0 < ys.length - 1; j0++) for (let j1 = j0; j1 < ys.length - 1; j1++) {
            if (!full(i0, i1, j0, j1)) continue;
            const grows = (i0 > 0 && full(i0 - 1, i1, j0, j1)) || (i1 + 1 < cells.length && full(i0, i1 + 1, j0, j1))
                || (j0 > 0 && full(i0, i1, j0 - 1, j1)) || (j1 + 2 < ys.length && full(i0, i1, j0, j1 + 1));
            if (!grows) boxes.push([i0, i1, j0, j1]);
        }
    // The largest new cover first, until the footprint is covered.
    const covered = cells.map(row => row.map(() => false));
    const uncovered = ([i0, i1, j0, j1]: number[]) => {
        let sum = 0;
        for (let i = i0; i <= i1; i++) for (let j = j0; j <= j1; j++) if (!covered[i][j]) sum += area(i, i, j, j);
        return sum;
    };
    const inner = cells.reduce((sum, row, i) => sum + row.reduce((r, cell, j) => r + (cell ? area(i, i, j, j) : 0), 0), 0);
    const candidates = boxes.filter(([i0, i1, j0, j1]) => Math.min(xs[i1 + 1] - xs[i0], ys[j1 + 1] - ys[j0]) >= NARROW_M || area(i0, i1, j0, j1) >= MISSED * inner);
    const chosen: number[][] = [];
    let missing = inner;
    while (missing > MISSED * inner && chosen.length < PARTS && candidates.length) {
        const best = candidates.reduce((a, b) => (uncovered(b) > uncovered(a) ? b : a));
        missing -= uncovered(best);
        chosen.push(best);
        for (let i = best[0]; i <= best[1]; i++) for (let j = best[2]; j <= best[3]; j++) covered[i][j] = true;
    }
    if (missing > MISSED * inner) return null;
    return chosen.length ? chosen.map(([i0, i1, j0, j1], k) => {
        const u = (xs[i0] + xs[i1 + 1]) / 2, v = (ys[j0] + ys[j1 + 1]) / 2, inset = 2 * k * INSET_M;
        return { x: fit.x + u * c - v * s, y: fit.y + u * s + v * c, width: xs[i1 + 1] - xs[i0] - inset, depth: ys[j1 + 1] - ys[j0] - inset };
    }) : null;
}

/**
 * The wall of a house's rectangle that faces `front` (east, south), + 1, as `front` of houses.wgsl numbers
 * them: 1 across −, 2 along +, 3 across +, 4 along −; 0 without a front.
 */
function door(angle: number, front?: [number, number]): number {
    if (!front) return 0;
    const along = front[0] * Math.cos(angle) - front[1] * Math.sin(angle), across = -front[0] * Math.sin(angle) - front[1] * Math.cos(angle);
    return Math.abs(along) >= Math.abs(across) ? (along > 0 ? 2 : 4) : across > 0 ? 3 : 1;
}

/** A stable seed from a point. */
const seedOf = (x: number, y: number) => (Math.imul(x, 73856093) ^ Math.imul(y, 19349663)) >>> 16;

/** The houses of feature tile `tile` whose rectangle's middle lies in the square of `owned`, a tile at or under it. */
export function tileHouses(list: Feature[], tile: Node, owned: Node = tile): TileHouses {
    const scale = unitScale(tile.level), [west, north, size] = square(tile, owned);
    const records: number[] = [], outlines: number[] = [], floats = new Float32Array(1), word = new Uint32Array(floats.buffer);
    const f32 = (v: number) => ((floats[0] = v), word[0]);
    const left = tile.x * EXTENT, top = tile.y * EXTENT, faces = fronts(list, tile);
    let building = -1;
    for (const feature of list) {
        if (feature.kind !== 'building') continue;
        // The wall that faces its road; fronts() lists the buildings in this order.
        const front = faces[++building];
        const tag = feature.tags.building ?? 'yes', part = feature.parts.find(p => p.outer);
        if (SKIPPED.has(tag) || !part || part.xy.length < 6) continue;
        // Metres east and north of the first vertex.
        const x0 = part.xy[0], y0 = part.xy[1], unit = metresPerUnit((top + y0) * scale), m = unit * scale;
        let ring: number[][] = [];
        for (let i = 0; i < part.xy.length; i += 2) ring.push([(part.xy[i] - x0) * m, -(part.xy[i + 1] - y0) * m]);
        if (twiceArea(ring) < 0) ring = ring.reverse();
        const area = twiceArea(ring) / 2, fit = rectangle(ring);
        if (area <= 0 || fit.width <= 0 || fit.depth <= 0) continue;
        const tx = x0 + fit.x / m, ty = y0 - fit.y / m;
        if (tx < west || tx >= west + size || ty < north || ty >= north + size) continue;
        const kind = kindOf(tag, area, feature.tags.landuse), levels = parseFloat(feature.tags['building:levels'] ?? ''), height = parseFloat(feature.tags.height ?? '');
        // A large footprint without a kind is a hall or a barn more often than flats.
        const low = LOW.has(tag) || kind === INDUSTRIAL || (kind === BLOCK && !BLOCKS.has(tag));
        const wall = levels > 0 ? levels * STOREY_M : height > 0 ? 0.65 * height : low ? 7 : 0;
        const pieces = area / (fit.width * fit.depth) >= RECTANGULAR ? [fit] : parts(ring, fit), outline = !pieces;
        // All parts of a building take the seed of its rectangle, so they share its colours and its pitch.
        const seed = seedOf(Math.round((left + tx) * scale), Math.round((top + ty) * scale));
        const flatRoof = feature.tags['roof:shape'] === 'flat', roof = ROOFS[feature.tags['roof:shape'] ?? ''] ?? 0;
        const index = records.length / RECORD_WORDS;
        for (const piece of pieces ?? [fit]) {
            const [L, W, angle] = piece.width >= piece.depth ? [piece.width, piece.depth, fit.angle] : [piece.depth, piece.width, fit.angle + Math.PI / 2];
            const x = Math.round((left + x0 + piece.x / m) * scale), y = Math.round((top + y0 - piece.y / m) * scale);
            records.push(x, y, f32(angle), f32(L), f32(W), f32(outline ? wall || (kind === GREENHOUSE ? GREENHOUSE_WALL_M : OUTLINE_WALL_M) : wall), f32(unit),
                (kind | (roof << 4) | (+outline << 6) | (+flatRoof << 7) | (door(angle, front) << 8) | (seed << 16)) >>> 0);
        }
        if (!outline) continue;
        // The walls, from the bottom to the top, with the metres along the outline; then the roof, whose vertices carry
        // their metres inward from the walls: houses.wgsl lifts them by the roof's pitch, or by nothing for a flat roof.
        const place = ([ex, ny]: number[]) => [Math.round((left + x0 + ex / m) * scale), Math.round((top + y0 - ny / m) * scale)];
        const world = ring.map(place);
        const vertex = ([wx, wy]: number[], up: number, roofed: number, u: number) => outlines.push(wx, wy, (index | (up << 31) | (roofed << 30)) >>> 0, f32(u));
        let along = 0;
        ring.forEach((a, i) => {
            const j = (i + 1) % ring.length, b = ring[j], length = Math.hypot(b[0] - a[0], b[1] - a[1]);
            for (const [k, up, u] of [[i, 0, along], [j, 0, along + length], [j, 1, along + length], [i, 0, along], [j, 1, along + length], [i, 1, along]]) vertex(world[k], up, 0, u);
            along += length;
        });
        // A roof that the map calls flat, and one about a courtyard, has no slopes.
        const hip = flatRoof || feature.parts.some(p => !p.outer) ? null : hipped(ring, Math.min(fit.width, fit.depth) / 2), cap = hip ? hip.top : ring, run = hip?.run ?? 0;
        if (hip) {
            const { walls, eaves, over, bands } = hip;
            // The underside of the eaves, then each band's slopes: the first starts at the eaves.
            if (over) walls.forEach((p, i) => {
                const j = (i + 1) % walls.length;
                for (const [q, u] of [[p, 0], [walls[j], 0], [eaves[j], -over], [p, 0], [eaves[j], -over], [eaves[i], -over]] as const) vertex(place(q), 1, 1, u);
            });
            bands.forEach(({ low, high, from, to }, k) => {
                const foot = k ? low : eaves, start = k ? from : -over;
                low.forEach((_, i) => {
                    const j = (i + 1) % low.length;
                    for (const [q, u] of [[foot[i], start], [foot[j], start], [high[j], to], [foot[i], start], [high[j], to], [high[i], to]] as const) vertex(place(q), 1, 1, u);
                });
            });
        }
        const triangles = earcut(cap.flat());
        for (let t = 0; t < triangles.length; t += 3) {
            const [a, b, c] = [triangles[t], triangles[t + 1], triangles[t + 2]];
            const turn = twiceArea([cap[a], cap[b], cap[c]]) >= 0 ? [a, b, c] : [a, c, b];
            for (const k of turn) vertex(place(cap[k]), 1, 1, run);
        }
    }
    return { records: new Uint32Array(records), outlines: new Uint32Array(outlines) };
}
