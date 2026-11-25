// The house templates: the faces of the prototype's buildings (src/models/buildings.ts) as vertices
// whose coordinates name slots, not numbers. houses.wgsl fills the slots of each house from its
// footprint, its style and its ground, so one template draws every house of its shape. A part that a
// house does not have, such as a second chimney, has slots that give its faces no area.

/** The templates, as `T_*` in houses.wgsl. */
export const TEMPLATES = ['gable', 'flat', 'spire', 'onion', 'domes', 'terrace', 'cones', 'granary', 'dormers'] as const;
export type Template = (typeof TEMPLATES)[number];

/** Rings bottom up, as radius and height in shares of the ring's unit: an onion dome, its lantern and its spike. */
const ONION = [[0.55, 0], [0.8, 0.35], [0.8, 0.7], [0.55, 1.05], [0.22, 1.35], [0.1, 1.6], [0.1, 1.9], [0.2, 2.05], [0.06, 2.3], [0, 2.9]];
/** A cupola on the ridge of a wooden church's nave: a drum from under the ridge, a cornice, and an onion dome. */
const CUPOLA = [[1, 0], [1, 1.6], [1.25, 1.6], [1.25, 1.85], ...ONION.map(([r, h]) => [r, h + 1.85])];
/** A trullo's cone of a bay: a drum, the stone cone at about 62°, and a pinnacle. */
const CONE = [[1, 0], [1, 0.2], [0, 2.1], [0.09, 2.21], [0, 2.37]];
/**
 * The simple level's rings, in four sides: points of the full rings, so that each lies inside its
 * full ring, which the full level draws over it.
 */
const SIMPLE_ONION = [[0.55, 0], [0.8, 0.5], [0.55, 1.05], [0.22, 1.35], [0.1, 1.6], [0, 2.9]];
const SIMPLE_CUPOLA = [[1, 0], [1, 1.6], ...SIMPLE_ONION.map(([r, h]) => [r, h + 1.85])];
const SIMPLE_CONE = [[1, 0], [0, 2.1]];
/**
 * On a chimney's shaft, in four sides: the underside, the side and the top of a slab that stands 7 cm past the
 * shaft. On the slab, in eight sides: a pot of clay and its mouth. The unit is half the shaft's side.
 */
const CHIMNEY_SLAB = [[1.41, 0], [1.74, 0], [1.74, 0.3], [0.3, 0.3]];
const CHIMNEY_POT = [[0.47, 0.3], [0.4, 1.3], [0, 1.3]];

/** Faces, as `M_*` in houses.wgsl: the fragment shader paints each by its material. */
export const WALL = 0, GABLE = 1, ROOF = 2, EDGE = 3, SOFFIT = 4, CHIMNEY = 5, CAP = 6, BALCONY = 7, TOWER = 8, FLAT = 9, RENDER = 10, GLASS = 11, FLOWERS = 12, GOLD = 13;

/**
 * A vertex: slots of x, y and z, each with a sign, offsets of x and z in centimetres, the material and
 * the wall. A ring vertex lies on a ring about its slots' point at the eighth `turn` of a full turn: the
 * ring's radius and height are shares of x slot `unit`, so a ring whose unit is 0 has no area.
 */
export interface Corner { x: number; y: number; z: number; sx?: number; sy?: number; dx?: number; dz?: number; ring?: [number, number]; unit?: number; turn?: number }

/**
 * Two words per vertex, as `template_at` in houses.wgsl reads them. Word 0: x slot (4 bits), y slot (its low
 * 4 bits), z slot (4), the signs of x and y (2), ring (1), material (4) at bit 15, wall (3) at bit 19, the
 * ring's unit slot (4) at bit 22, turn (3) at bit 26 and the fifth bit of the y slot at bit 29. Word 1: the
 * offset of z in centimetres, signed 16 bits; at bit 16 the offset of x, signed 16 bits, or on a ring its
 * radius in hundredths (8 bits) and its height in fiftieths (8).
 */
export const VERTEX_WORDS = 2;

/** A template: its vertices, three per triangle, and how many of them the simple level of detail draws. */
export interface Mesh { words: Uint32Array; simple: number; full: number }

class Builder {
    private readonly words: number[] = [];
    simple = 0;

    /** A convex polygon, counter-clockwise seen from outside. */
    poly(corners: Corner[], material: number, wall = 0): void {
        for (let i = 1; i + 1 < corners.length; i++) for (const c of [corners[0], corners[i], corners[i + 1]]) this.vertex(c, material, wall);
    }

    /** A roof plane given by its underside, counter-clockwise from above, 30 cm thick: its top, and in full detail its underside and edges. */
    slab(under: Corner[], tops: boolean, material = ROOF): void {
        const over = under.map(c => ({ ...c, dz: (c.dz ?? 0) + 30 }));
        if (tops) return this.poly(over, material);
        this.poly([...under].reverse(), SOFFIT);
        under.forEach((c, i) => this.poly([c, under[(i + 1) % under.length], over[(i + 1) % under.length], over[i]], EDGE));
    }

    /**
     * A surface of revolution in eight sides, or four, about `at`: each step of `profile` a band of `materials[step]`.
     * With `square`, four sides stand along the house's own sides.
     */
    rings(at: Corner, profile: number[][], materials: number[], sides: 4 | 8 = 8, square = false): void {
        const step = 8 / sides;
        for (let k = 0; k + 1 < profile.length; k++) for (let i = +square; i < 8; i += step) {
            const c = (ring: number, turn: number): Corner => ({ ...at, ring: profile[ring] as [number, number], turn: turn % 8 });
            this.poly([c(k, i), c(k, i + step), c(k + 1, i + step), c(k + 1, i)], materials[k]);
        }
    }

    /** The end of the simple level of detail. */
    mark(): void {
        this.simple = this.words.length / VERTEX_WORDS;
    }

    mesh(): Mesh {
        return { words: new Uint32Array(this.words), simple: this.simple, full: this.words.length / VERTEX_WORDS };
    }

    private vertex({ x, y, z, sx = 1, sy = 1, dx = 0, dz = 0, ring, unit = 0, turn = 0 }: Corner, material: number, wall: number): void {
        const high = ring ? (Math.round(ring[0] * 100) << 16) | (Math.round(ring[1] * 50) << 24) : (dx & 0xffff) << 16;
        this.words.push((x | ((y & 15) << 4) | ((y >> 4) << 29) | (z << 8) | (+(sx < 0) << 12) | (+(sy < 0) << 13) | (+!!ring << 14) | (material << 15) | (wall << 19) | (unit << 22) | (turn << 26)) >>> 0,
            ((dz & 0xffff) | high) >>> 0);
    }
}

/** The four walls from z slot `bottom` to `top`, counter-clockwise from above, the first along −y: x slot `x`, y slot `y`. */
function walls(b: Builder, x: number, y: number, bottom: number, top: number, material = WALL): void {
    const corners: [number, number][] = [[-1, -1], [1, -1], [1, 1], [-1, 1]];
    corners.forEach(([ax, ay], w) => {
        const [bx, by] = corners[(w + 1) % 4];
        b.poly([{ x, y, z: bottom, sx: ax, sy: ay }, { x, y, z: bottom, sx: bx, sy: by }, { x, y, z: top, sx: bx, sy: by }, { x, y, z: top, sx: ax, sy: ay }], material, w);
    });
}

/** A rectangle of x slot `x` and y slot `y` at z slot `z`, facing up, or down when `down`. */
function rectangle(x: number, y: number, z: number, down = false): Corner[] {
    const ring = [[-1, -1], [1, -1], [1, 1], [-1, 1]].map(([sx, sy]) => ({ x, y, z, sx, sy }));
    return down ? ring.reverse() : ring;
}

/** A slot with its sign. */
type Bound = [number, number];

/** A box from x0 to x1, y0 to y1 (x0 < x1, y0 < y1) and z slots z0 to z1, with its top in `top` and a floor in `floor`. */
function box(b: Builder, [x0, x1]: [Bound, Bound], [y0, y1]: [Bound, Bound], [z0, z1]: [number, number], side: number, top: number, floor?: number, dz0 = 0): void {
    const ring = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(([[x, sx], [y, sy]]) => ({ x, sx, y, sy }));
    const at = (c: Omit<Corner, 'z'>, z: number, dz = 0) => ({ ...c, z, dz });
    if (floor !== undefined) b.poly(ring.map(c => at(c, z0, dz0)).reverse(), floor);
    ring.forEach((c, i) => {
        const d = ring[(i + 1) % 4];
        b.poly([at(c, z0, dz0), at(d, z0, dz0), at(d, z1), at(c, z1)], side);
    });
    b.poly(ring.map(c => at(c, z1)), top);
}

/** The slots of a pitched roof over walls: x L/2, the roof's end and the ridge's end; y W/2, the eaves, the hip's foot and the gable's top; z the walls' foot and top, the eaves, the ridge, the hip and the gable's top. */
interface Pitched { X: { HALF: number; END: number; RIDGE: number }; Y: { HALF: number; EAVE: number; HIP: number; TOP: number }; Z: { FOOT: number; WALL: number; EAVE: number; RIDGE: number; HIP: number; TOP: number } }

/** Walls and gables, the tops of a gabled or hipped roof, what `simple` adds, the end of the simple level, and the roof's undersides. */
function pitched(b: Builder, { X, Y, Z }: Pitched, simple = () => {}): void {
    const roof = (tops: boolean) => {
        for (const s of [1, -1]) {
            // A side of the roof, mirrored by s through the ridge and the middle.
            const under: Corner[] = [
                { x: X.END, y: Y.EAVE, z: Z.EAVE, sx: -s, sy: -s }, { x: X.END, y: Y.EAVE, z: Z.EAVE, sx: s, sy: -s },
                { x: X.END, y: Y.HIP, z: Z.HIP, sx: s, sy: -s }, { x: X.RIDGE, y: 0, z: Z.RIDGE, sx: s },
                { x: X.RIDGE, y: 0, z: Z.RIDGE, sx: -s }, { x: X.END, y: Y.HIP, z: Z.HIP, sx: -s, sy: -s },
            ];
            b.slab(under, tops);
            b.slab([{ x: X.END, y: Y.HIP, z: Z.HIP, sx: s, sy: -s }, { x: X.END, y: Y.HIP, z: Z.HIP, sx: s, sy: s }, { x: X.RIDGE, y: 0, z: Z.RIDGE, sx: s }], tops);
        }
    };
    walls(b, X.HALF, Y.HALF, Z.FOOT, Z.WALL);
    for (const s of [1, -1]) {
        b.poly([{ x: X.HALF, y: Y.HALF, z: Z.WALL, sx: s, sy: -s }, { x: X.HALF, y: Y.HALF, z: Z.WALL, sx: s, sy: s }, { x: X.HALF, y: Y.TOP, z: Z.TOP, sx: s, sy: s }, { x: X.HALF, y: Y.TOP, z: Z.TOP, sx: s, sy: -s }], GABLE);
    }
    roof(true);
    simple();
    b.mark();
    roof(false);
}

/** The z slots of every template, as `Z_*` in houses.wgsl: 0 is the bottom of the walls, under the ground. */
export const Z = { BOTTOM: 0, WALL: 1 } as const;

/**
 * Slots of the gable template, as `slots_of` in houses.wgsl fills them. x: L/2, the roof's end, the ridge's
 * end, the two sides of each chimney, each balcony's half-length, the middle of the outer dormers, half
 * the side of each chimney, the middle of the chimneys, and half the length of each balcony's flower box.
 * y: W/2, the eaves, the hip's foot, the gable's top, a chimney's half depth, a slot without use, then for
 * the middle dormer and for the outer pair the front, the front of the eaves and the window, half the ridge
 * cap, and for each balcony its outer side, the inner side of its front board and the front of its flower
 * box. z: the eaves, the ridge, the hip, the gable's top, the chimneys' tops, the balconies' floors and
 * railing tops, and the top of the dormers' walls and their apex. A dormer that a house has not lies flat
 * in the roof's middle.
 */
const G = {
    X: { HALF: 1, END: 2, RIDGE: 3, C1A: 4, C1B: 5, C2A: 6, C2B: 7, B1: 8, B2: 9, DORMERS: 10, C1: 11, C2: 12, CHIMNEYS: 13, F1: 14, F2: 15 },
    Y: { HALF: 1, EAVE: 2, HIP: 3, TOP: 4, CHIMNEY: 5, MIDDLE: 7, OUTER: 10, CAP: 13, B1: 16, B2: 19 },
    Z: { EAVE: 2, RIDGE: 3, HIP: 4, TOP: 5, C1: 6, C2: 7, B1A: 8, B1B: 9, B2A: 10, B2B: 11, MIDDLE: 12, OUTER: 14 },
};

/** A gabled dormer in the front wall about x slot `x` (sign `sx`): y slots front, eaves and window from `y`, z slots top and apex from `z`. */
function dormer(b: Builder, x: number, sx: number, y: number, z: number): void {
    const [top, apex] = [z, z + 1], [front, eave, glass] = [y, y + 1, y + 2], at = (dx: number, yy: number, zz: number, dz = 0): Corner => ({ x, sx, dx, y: yy, sy: -1, z: zz, dz });
    b.poly([at(-65, front, Z.WALL), at(65, front, Z.WALL), at(65, front, top), at(0, front, apex), at(-65, front, top)], GABLE);
    b.poly([at(-40, glass, Z.WALL, 40), at(40, glass, Z.WALL, 40), at(40, glass, top, -15), at(-40, glass, top, -15)], GLASS);
    // The cheeks and the roof run back to the ridge's line, under the main roof.
    b.poly([at(-65, 0, Z.WALL), at(-65, front, Z.WALL), at(-65, front, top), at(-65, 0, top)], GABLE);
    b.poly([at(65, front, Z.WALL), at(65, 0, Z.WALL), at(65, 0, top), at(65, front, top)], GABLE);
    for (const tops of [true, false]) {
        b.slab([at(-90, eave, top, -30), at(0, eave, apex), at(0, 0, apex), at(-90, 0, top, -30)], tops);
        b.slab([at(0, eave, apex), at(90, eave, top, -30), at(90, 0, top, -30), at(0, 0, apex)], tops);
    }
}

/**
 * A balcony before the front wall: a floor SLAB_CM thick and a railing of boards BOARD_CM thick on its three open
 * sides, with a flower box along the front board's top. x slot `x` is half its length and `flowers` half the flower
 * box's; y slots `wall`, and from `y` its outer side, the inner side of its front board and the front of the flower
 * box; z slots `floor`, the floor's top, and `top`, the railing's top. A house without it gives it no length, no
 * depth and a railing of no height, and one without flowers gives the flower box no length and no depth.
 */
function balcony(b: Builder, x: number, flowers: number, wall: number, y: number, floor: number, top: number): void {
    const SLAB_CM = 15, BOARD_CM = 6, BOX_CM = 16, [out, inner, planter] = [y, y + 1, y + 2];
    const at = (sx: number, dx: number, ys: number, z: number, dz = 0): Corner => ({ x, sx, dx: sx * dx, y: ys, sy: -1, z, dz });
    // The outside: the front and the two ends, from the floor's underside to the railing's top; then the underside.
    b.poly([at(-1, 0, out, floor, -SLAB_CM), at(1, 0, out, floor, -SLAB_CM), at(1, 0, out, top), at(-1, 0, out, top)], BALCONY);
    for (const s of [1, -1]) {
        const end = [at(s, 0, out, floor, -SLAB_CM), at(s, 0, wall, floor, -SLAB_CM), at(s, 0, wall, top), at(s, 0, out, top)];
        b.poly(s > 0 ? end : [end[1], end[0], end[3], end[2]], BALCONY);
    }
    b.poly([at(-1, 0, wall, floor, -SLAB_CM), at(1, 0, wall, floor, -SLAB_CM), at(1, 0, out, floor, -SLAB_CM), at(-1, 0, out, floor, -SLAB_CM)], SOFFIT);
    // The tops of the boards, their inner faces, and the floor between them.
    b.poly([at(-1, 0, out, top), at(1, 0, out, top), at(1, 0, inner, top), at(-1, 0, inner, top)], BALCONY);
    for (const s of [1, -1]) {
        const strip = [at(s, -BOARD_CM, inner, top), at(s, 0, inner, top), at(s, 0, wall, top), at(s, -BOARD_CM, wall, top)];
        b.poly(s > 0 ? strip : strip.reverse(), BALCONY);
        const face = [at(s, -BOARD_CM, wall, floor), at(s, -BOARD_CM, inner, floor), at(s, -BOARD_CM, inner, top), at(s, -BOARD_CM, wall, top)];
        b.poly(s > 0 ? face : face.reverse(), BALCONY);
    }
    b.poly([at(1, -BOARD_CM, inner, floor), at(-1, -BOARD_CM, inner, floor), at(-1, -BOARD_CM, inner, top), at(1, -BOARD_CM, inner, top)], BALCONY);
    b.poly([at(-1, -BOARD_CM, inner, floor), at(1, -BOARD_CM, inner, floor), at(1, -BOARD_CM, wall, floor), at(-1, -BOARD_CM, wall, floor)], SOFFIT);
    // The flower box hangs before the front board, and the flowers stand out of it.
    const of = (sx: number, ys: number, dz: number): Corner => ({ x: flowers, sx, y: ys, sy: -1, z: top, dz });
    for (const [z0, z1, side] of [[-BOX_CM, 0, BALCONY], [0, 14, FLOWERS]]) {
        b.poly([of(-1, planter, z0), of(1, planter, z0), of(1, planter, z1), of(-1, planter, z1)], side);
        for (const s of [1, -1]) b.poly(s > 0 ? [of(s, planter, z0), of(s, out, z0), of(s, out, z1), of(s, planter, z1)] : [of(s, out, z0), of(s, planter, z0), of(s, planter, z1), of(s, out, z1)], side);
    }
    b.poly([of(-1, planter, 14), of(1, planter, 14), of(1, out, 14), of(-1, out, 14)], FLOWERS);
    b.poly([of(1, out, 0), of(-1, out, 0), of(-1, out, 14), of(1, out, 14)], FLOWERS);
    b.poly([of(-1, out, -BOX_CM), of(1, out, -BOX_CM), of(1, planter, -BOX_CM), of(-1, planter, -BOX_CM)], SOFFIT);
}

function gable(dormers: boolean): Mesh {
    const b = new Builder(), { X, Y, Z: GZ } = G;
    pitched(b, { X, Y, Z: { FOOT: Z.BOTTOM, WALL: Z.WALL, EAVE: GZ.EAVE, RIDGE: GZ.RIDGE, HIP: GZ.HIP, TOP: GZ.TOP } });
    const chimney: [Bound, Bound] = [[Y.CHIMNEY, -1], [Y.CHIMNEY, 1]];
    // A chimney stands from 1 m under the ridge.
    box(b, [[X.C1A, 1], [X.C1B, 1]], chimney, [GZ.RIDGE, GZ.C1], CHIMNEY, CAP, undefined, -100);
    box(b, [[X.C2B, -1], [X.C2A, -1]], chimney, [GZ.RIDGE, GZ.C2], CHIMNEY, CAP, undefined, -100);
    // On each shaft a slab and a pot.
    for (const [sx, z, unit] of [[1, GZ.C1, X.C1], [-1, GZ.C2, X.C2]]) {
        const at = { x: X.CHIMNEYS, sx, y: 0, z, unit };
        b.rings(at, CHIMNEY_SLAB, [CAP, CAP, CAP], 4, true);
        b.rings(at, CHIMNEY_POT, [ROOF, EDGE]);
    }
    // The ridge cap: a low tent along the ridge, its feet in the steepest roof, its ends 3 cm short of the roof's.
    // A roof without a ridge gives it no width.
    for (const s of [1, -1]) {
        const foot = (sx: number, sy: number): Corner => ({ x: X.RIDGE, sx, dx: -3 * sx, y: Y.CAP, sy, z: GZ.RIDGE, dz: -4 }), top = (sx: number): Corner => ({ x: X.RIDGE, sx, dx: -3 * sx, y: 0, z: GZ.RIDGE, dz: 39 });
        b.poly([foot(-s, -s), foot(s, -s), top(s), top(-s)], EDGE);
        b.poly([foot(s, -s), foot(s, s), top(s)], EDGE);
    }
    balcony(b, X.B1, X.F1, Y.HALF, Y.B1, GZ.B1A, GZ.B1B);
    balcony(b, X.B2, X.F2, Y.HALF, Y.B2, GZ.B2A, GZ.B2B);
    if (dormers) {
        dormer(b, 0, 1, Y.MIDDLE, GZ.MIDDLE);
        for (const sx of [1, -1]) dormer(b, X.DORMERS, sx, Y.OUTER, GZ.OUTER);
    }
    return b.mesh();
}

/** Slots of the flat template, as `slots_of` in houses.wgsl fills them: the walls' half sides, and the roof's 20 cm past them. */
function flat(): Mesh {
    const b = new Builder();
    walls(b, 1, 1, Z.BOTTOM, Z.WALL);
    const roof = rectangle(2, 2, Z.WALL);
    b.slab(roof, true, FLAT);
    b.mark();
    b.slab(roof, false, FLAT);
    return b.mesh();
}

/** Slots of the terrace template: x and y the walls' half sides and the parapet's inner side; z the roof, and the parapet's top. */
function terrace(): Mesh {
    const b = new Builder(), TOP = 2;
    walls(b, 1, 1, Z.BOTTOM, TOP);
    // The parapet's inner faces look into the roof.
    const inner = rectangle(2, 2, Z.WALL), outer = rectangle(1, 1, TOP);
    inner.forEach((c, i) => {
        const d = inner[(i + 1) % 4];
        b.poly([d, c, { ...c, z: TOP }, { ...d, z: TOP }], RENDER);
        b.poly([outer[i], outer[(i + 1) % 4], { ...d, z: TOP }, { ...c, z: TOP }], RENDER);
    });
    b.poly(inner, ROOF);
    b.mark();
    return b.mesh();
}

/**
 * Slots of the cones template, which holds three columns and three rows of cones: a middle one and an
 * outer pair of each. x: L/2, the outer columns, and the cones' radius in the middle column and row, in
 * an outer column and the middle row, in the middle column and an outer row, and in both outer; y W/2
 * and the outer rows. A column or row that a house has not has cones of radius 0.
 */
function cones(): Mesh {
    const b = new Builder();
    walls(b, 1, 1, Z.BOTTOM, Z.WALL);
    b.poly(rectangle(1, 1, Z.WALL), RENDER);
    const each = (cone: (at: Corner) => void) => {
        for (const [x, sxs] of [[0, [1]], [2, [1, -1]]] as const) for (const [y, sys] of [[0, [1]], [2, [1, -1]]] as const)
            for (const sx of sxs) for (const sy of sys) cone({ x, y, z: Z.WALL, sx, sy, unit: 3 + (x ? 1 : 0) + (y ? 2 : 0) });
    };
    each(at => b.rings(at, SIMPLE_CONE, [ROOF], 4));
    b.mark();
    each(at => b.rings(at, CONE, [RENDER, ROOF, RENDER, RENDER]));
    return b.mesh();
}

/**
 * Slots of the granary template: a store on a row of up to seven pillars under each long wall. x L/2,
 * the roof's end, the ridge's end, the middle pillar's half side, then the inner and the outer side of
 * each of three pairs of pillars about the middle; y W/2, the eaves, the hip's foot, the gable's top, a
 * pillar's inner and outer side; z the pillars' foot, the store's floor, its eaves, the roof's eaves, the
 * ridge, the hip and the gable's top. A pillar that a granary has not has no width.
 */
function granary(): Mesh {
    const b = new Builder();
    pitched(b, { X: { HALF: 1, END: 2, RIDGE: 3 }, Y: { HALF: 1, EAVE: 2, HIP: 3, TOP: 4 }, Z: { FOOT: 1, WALL: 2, EAVE: 3, RIDGE: 4, HIP: 5, TOP: 6 } }, () => {
        b.poly(rectangle(1, 1, 1, true), SOFFIT);
        for (const sy of [1, -1]) {
            const y: [Bound, Bound] = sy > 0 ? [[5, 1], [6, 1]] : [[6, -1], [5, -1]];
            box(b, [[4, -1], [4, 1]], y, [Z.BOTTOM, 1], CAP, CAP);
            for (const x of [5, 7, 9]) {
                box(b, [[x, 1], [x + 1, 1]], y, [Z.BOTTOM, 1], CAP, CAP);
                box(b, [[x + 1, -1], [x, -1]], y, [Z.BOTTOM, 1], CAP, CAP);
            }
        }
    });
    return b.mesh();
}

/**
 * Slots of the church templates, as `slots_of` in houses.wgsl fills them, with x signed: the nave's west and
 * east walls and roof ends, the tower's west side, its middle, the spire's eaves, half the tower's side, and
 * the middle and the unit of each cupola. y: W/2, the eaves, the tower's half side, the spire's eaves, and half the
 * thickness and half the arms of the cross. z: the eaves, the ridge, the tower's top, the spire's eaves and its apex,
 * the cupolas' foot under the ridge, and the foot of the cross.
 */
const C = {
    X: { NAVE_W: 1, NAVE_E: 2, END_W: 3, END_E: 4, TOWER_W: 5, TOWER: 6, SPIRE_W: 7, SPIRE_E: 8, UNIT: 9, CUPOLA: [10, 11], CUPOLA_UNIT: [12, 13] },
    Y: { HALF: 1, EAVE: 2, TOWER: 3, SPIRE: 4, ZERO: 0, BAR: 5, ARM: 6 },
    Z: { EAVE: 2, RIDGE: 3, TOP: 4, SPIRE: 5, APEX: 6, CUPOLA: 7, CROSS: 8 },
};

function church(top: 'spire' | 'onion' | 'domes'): Mesh {
    const b = new Builder(), { X, Y, Z: CZ } = C;
    const nave = (tops: boolean) => {
        b.slab([{ x: X.END_W, y: Y.EAVE, z: CZ.EAVE, sy: -1 }, { x: X.END_E, y: Y.EAVE, z: CZ.EAVE, sy: -1 }, { x: X.END_E, y: Y.ZERO, z: CZ.RIDGE }, { x: X.END_W, y: Y.ZERO, z: CZ.RIDGE }], tops);
        b.slab([{ x: X.END_E, y: Y.EAVE, z: CZ.EAVE }, { x: X.END_W, y: Y.EAVE, z: CZ.EAVE }, { x: X.END_W, y: Y.ZERO, z: CZ.RIDGE }, { x: X.END_E, y: Y.ZERO, z: CZ.RIDGE }], tops);
    };
    const spire = (tops: boolean) => {
        if (top !== 'spire') {
            // A cornice caps the tower, and the dome rises from it.
            b.slab([[X.SPIRE_W, -1], [X.SPIRE_E, -1], [X.SPIRE_E, 1], [X.SPIRE_W, 1]].map(([x, sy]) => ({ x, y: Y.SPIRE, z: CZ.TOP, sy })), tops);
            // The simple level draws the domes in four sides, the full level over them in eight.
            const [onion, cupola, sides] = tops ? [SIMPLE_ONION, SIMPLE_CUPOLA, 4 as const] : [ONION, CUPOLA, 8 as const];
            b.rings({ x: X.TOWER, y: Y.ZERO, z: CZ.TOP, dz: 30, unit: X.UNIT }, onion, onion.map(() => ROOF), sides);
            if (top === 'domes') X.CUPOLA.forEach((x, k) => b.rings({ x, y: Y.ZERO, z: CZ.CUPOLA, unit: X.CUPOLA_UNIT[k] }, cupola, cupola.map((_, i) => (i === 0 ? GABLE : ROOF)), sides));
            return;
        }
        const ring: Corner[] = [[X.SPIRE_W, -1], [X.SPIRE_E, -1], [X.SPIRE_E, 1], [X.SPIRE_W, 1]].map(([x, sy]) => ({ x, y: Y.SPIRE, z: CZ.SPIRE, sy }));
        ring.forEach((c, i) => b.slab([c, ring[(i + 1) % 4], { x: X.TOWER, y: Y.ZERO, z: CZ.APEX }], tops));
    };
    // The tower, then the nave's walls and gables.
    const tower: Corner[] = [[X.TOWER_W, -1], [X.NAVE_W, -1], [X.NAVE_W, 1], [X.TOWER_W, 1]].map(([x, sy]) => ({ x, y: Y.TOWER, z: Z.BOTTOM, sy }));
    tower.forEach((c, w) => {
        const d = tower[(w + 1) % 4];
        b.poly([c, d, { ...d, z: CZ.TOP }, { ...c, z: CZ.TOP }], TOWER, w);
    });
    const naveCorners: Corner[] = [[X.NAVE_W, -1], [X.NAVE_E, -1], [X.NAVE_E, 1], [X.NAVE_W, 1]].map(([x, sy]) => ({ x, y: Y.HALF, z: Z.BOTTOM, sy }));
    naveCorners.forEach((c, w) => {
        const d = naveCorners[(w + 1) % 4];
        b.poly([c, d, { ...d, z: Z.WALL }, { ...c, z: Z.WALL }], WALL, w);
    });
    for (const [x, s] of [[X.NAVE_E, 1], [X.NAVE_W, -1]]) b.poly([{ x, y: Y.HALF, z: Z.WALL, sy: -s }, { x, y: Y.HALF, z: Z.WALL, sy: s }, { x, y: Y.ZERO, z: CZ.RIDGE }], GABLE);
    nave(true);
    spire(true);
    b.mark();
    nave(false);
    spire(false);
    // The cross on the tower's top: an upright from 30 cm inside the top, and the arms across the church, a little
    // thinner than the upright, so that the two share no plane.
    const bar = (y: number, z0: number, z1: number, half: number): void => {
        const at = (sx: number, sy: number, dz: number): Corner => ({ x: X.TOWER, dx: half * sx, y, sy, z: CZ.CROSS, dz });
        const ring: [number, number][] = [[-1, -1], [1, -1], [1, 1], [-1, 1]];
        ring.forEach(([ax, ay], i) => {
            const [bx, by] = ring[(i + 1) % 4];
            b.poly([at(ax, ay, z0), at(bx, by, z0), at(bx, by, z1), at(ax, ay, z1)], GOLD);
        });
        b.poly(ring.map(([sx, sy]) => at(sx, sy, z1)), GOLD);
        b.poly(ring.map(([sx, sy]) => at(sx, sy, z0)).reverse(), GOLD);
    };
    bar(Y.BAR, -30, 150, 5);
    bar(Y.ARM, 92, 100, 4);
    return b.mesh();
}

/** Every template, in the order of TEMPLATES. */
export const MESHES: Record<Template, Mesh> = {
    gable: gable(false), flat: flat(), spire: church('spire'), onion: church('onion'), domes: church('domes'), terrace: terrace(), cones: cones(), granary: granary(), dormers: gable(true),
};
