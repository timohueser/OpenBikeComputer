// The house templates: the faces of the prototype's buildings (src/models/buildings.ts) as vertices
// whose coordinates name slots, not numbers. houses.wgsl fills the slots of each house from its
// footprint, its style and its ground, so one template draws every house of its shape. A part that a
// house does not have, such as a second chimney, has slots that give its faces no area.

/** The templates, as `T_*` in houses.wgsl. */
export const TEMPLATES = ['gable', 'flat', 'spire', 'onion', 'domes', 'terrace', 'cones', 'granary'] as const;
export type Template = (typeof TEMPLATES)[number];

/** Rings bottom up, as radius and height in shares of the ring's unit: an onion dome, its lantern and its spike. */
const ONION = [[0.55, 0], [0.8, 0.35], [0.8, 0.7], [0.55, 1.05], [0.22, 1.35], [0.1, 1.6], [0.1, 1.9], [0.2, 2.05], [0.06, 2.3], [0, 2.9]];
/** A cupola on the ridge of a wooden church's nave: a drum from under the ridge, a cornice, and an onion dome. */
const CUPOLA = [[1, 0], [1, 1.6], [1.25, 1.6], [1.25, 1.85], ...ONION.map(([r, h]) => [r, h + 1.85])];
/** A trullo's cone of a bay: a drum, the stone cone at about 62°, and a pinnacle. */
const CONE = [[1, 0], [1, 0.2], [0, 2.1], [0.09, 2.21], [0, 2.37]];

/** Faces, as `M_*` in houses.wgsl: the fragment shader paints each by its material. */
export const WALL = 0, GABLE = 1, ROOF = 2, EDGE = 3, SOFFIT = 4, CHIMNEY = 5, CAP = 6, BALCONY = 7, TOWER = 8, FLAT = 9, RENDER = 10, GLASS = 11;

/**
 * A vertex: slots of x, y and z, each with a sign, offsets of x and z in centimetres, the material and
 * the wall. A ring vertex lies on a ring about its slots' point at the eighth `turn` of a full turn: the
 * ring's radius and height are shares of x slot `unit`, so a ring whose unit is 0 has no area.
 */
export interface Corner { x: number; y: number; z: number; sx?: number; sy?: number; dx?: number; dz?: number; ring?: [number, number]; unit?: number; turn?: number }

/**
 * Two words per vertex, as `template_at` in houses.wgsl reads them. Word 0: x slot (4 bits), y slot (4),
 * z slot (4), the signs of x and y (2), ring (1), material (4) at bit 15, wall (3) at bit 19, the ring's
 * unit slot (4) at bit 22 and turn (3) at bit 26. Word 1: the offset of z in centimetres, signed 16 bits;
 * at bit 16 the offset of x, signed 16 bits, or on a ring its radius in hundredths (8 bits) and its height
 * in fiftieths (8).
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

    /** A surface of revolution in eight sides about `at`: each step of `profile` a band of `materials[step]`. */
    rings(at: Corner, profile: number[][], materials: number[]): void {
        for (let k = 0; k + 1 < profile.length; k++) for (let i = 0; i < 8; i++) {
            const c = (ring: number, turn: number): Corner => ({ ...at, ring: profile[ring] as [number, number], turn: turn % 8 });
            this.poly([c(k, i), c(k, i + 1), c(k + 1, i + 1), c(k + 1, i)], materials[k]);
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
        this.words.push((x | (y << 4) | (z << 8) | (+(sx < 0) << 12) | (+(sy < 0) << 13) | (+!!ring << 14) | (material << 15) | (wall << 19) | (unit << 22) | (turn << 26)) >>> 0,
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
 * end, the two sides of each chimney, each balcony's half-length, and the middle of the outer dormers. y:
 * W/2, the eaves, the hip's foot, the gable's top, a chimney's half depth, a balcony's outer side, then
 * for the middle dormer and for the outer pair the front, the front of the eaves and the window. z: the
 * eaves, the ridge, the hip, the gable's top, the chimneys' tops, the balconies' floors and tops, and the
 * top of the dormers' walls and their apex. A dormer that a house has not lies flat in the roof's middle.
 */
const G = {
    X: { HALF: 1, END: 2, RIDGE: 3, C1A: 4, C1B: 5, C2A: 6, C2B: 7, B1: 8, B2: 9, DORMERS: 10 },
    Y: { HALF: 1, EAVE: 2, HIP: 3, TOP: 4, CHIMNEY: 5, BALCONY: 6, MIDDLE: 7, OUTER: 10 },
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

function gable(): Mesh {
    const b = new Builder(), { X, Y, Z: GZ } = G;
    pitched(b, { X, Y, Z: { FOOT: Z.BOTTOM, WALL: Z.WALL, EAVE: GZ.EAVE, RIDGE: GZ.RIDGE, HIP: GZ.HIP, TOP: GZ.TOP } });
    const chimney: [Bound, Bound] = [[Y.CHIMNEY, -1], [Y.CHIMNEY, 1]], balcony: [Bound, Bound] = [[Y.BALCONY, -1], [Y.HALF, -1]];
    // A chimney stands from 1 m under the ridge.
    box(b, [[X.C1A, 1], [X.C1B, 1]], chimney, [GZ.RIDGE, GZ.C1], CHIMNEY, CAP, undefined, -100);
    box(b, [[X.C2B, -1], [X.C2A, -1]], chimney, [GZ.RIDGE, GZ.C2], CHIMNEY, CAP, undefined, -100);
    box(b, [[X.B1, -1], [X.B1, 1]], balcony, [GZ.B1A, GZ.B1B], BALCONY, BALCONY, SOFFIT);
    box(b, [[X.B2, -1], [X.B2, 1]], balcony, [GZ.B2A, GZ.B2B], BALCONY, BALCONY, SOFFIT);
    dormer(b, 0, 1, Y.MIDDLE, GZ.MIDDLE);
    for (const sx of [1, -1]) dormer(b, X.DORMERS, sx, Y.OUTER, GZ.OUTER);
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
 * Slots of the cones template: x L/2, the outer columns of cones, and the cones' radius in the middle
 * column and row, in an outer column and the middle row, in the middle column and an outer row, and in
 * both outer; y W/2 and the outer rows. A column or row that a house has not has cones of radius 0.
 */
function cones(): Mesh {
    const b = new Builder();
    walls(b, 1, 1, Z.BOTTOM, Z.WALL);
    b.poly(rectangle(1, 1, Z.WALL), RENDER);
    for (const [x, sxs] of [[0, [1]], [2, [1, -1]]] as const) for (const [y, sys] of [[0, [1]], [2, [1, -1]]] as const)
        for (const sx of sxs) for (const sy of sys) b.rings({ x, y, z: Z.WALL, sx, sy, unit: 3 + (x ? 1 : 0) + (y ? 2 : 0) }, CONE, [RENDER, ROOF, RENDER, RENDER]);
    b.mark();
    return b.mesh();
}

/**
 * Slots of the granary template: a store on two rows of three pillars. x L/2, the roof's end, the ridge's
 * end, the middle pillar's half side, an end pillar's inner and outer side; y W/2, the eaves, the hip's
 * foot, the gable's top, a pillar's inner and outer side; z the pillars' foot, the store's floor, its
 * eaves, the roof's eaves, the ridge, the hip and the gable's top.
 */
function granary(): Mesh {
    const b = new Builder();
    pitched(b, { X: { HALF: 1, END: 2, RIDGE: 3 }, Y: { HALF: 1, EAVE: 2, HIP: 3, TOP: 4 }, Z: { FOOT: 1, WALL: 2, EAVE: 3, RIDGE: 4, HIP: 5, TOP: 6 } }, () => {
        b.poly(rectangle(1, 1, 1, true), SOFFIT);
        for (const sy of [1, -1]) {
            const y: [Bound, Bound] = sy > 0 ? [[5, 1], [6, 1]] : [[6, -1], [5, -1]];
            box(b, [[4, -1], [4, 1]], y, [Z.BOTTOM, 1], CAP, CAP);
            box(b, [[5, 1], [6, 1]], y, [Z.BOTTOM, 1], CAP, CAP);
            box(b, [[6, -1], [5, -1]], y, [Z.BOTTOM, 1], CAP, CAP);
        }
    });
    return b.mesh();
}

/**
 * Slots of the church templates, as `slots_of` in houses.wgsl fills them, with x signed: the nave's west and
 * east walls and roof ends, the tower's west side, its middle, the spire's eaves, half the tower's side, and
 * the middle and the unit of each cupola. y: W/2, the eaves, the tower's half side and the spire's eaves. z:
 * the eaves, the ridge, the tower's top, the spire's eaves and its apex, and the cupolas' foot under the ridge.
 */
const C = {
    X: { NAVE_W: 1, NAVE_E: 2, END_W: 3, END_E: 4, TOWER_W: 5, TOWER: 6, SPIRE_W: 7, SPIRE_E: 8, UNIT: 9, CUPOLA: [10, 11], CUPOLA_UNIT: [12, 13] },
    Y: { HALF: 1, EAVE: 2, TOWER: 3, SPIRE: 4, ZERO: 0 },
    Z: { EAVE: 2, RIDGE: 3, TOP: 4, SPIRE: 5, APEX: 6, CUPOLA: 7 },
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
            if (!tops) return;
            b.rings({ x: X.TOWER, y: Y.ZERO, z: CZ.TOP, dz: 30, unit: X.UNIT }, ONION, ONION.map(() => ROOF));
            if (top === 'domes') X.CUPOLA.forEach((x, k) => b.rings({ x, y: Y.ZERO, z: CZ.CUPOLA, unit: X.CUPOLA_UNIT[k] }, CUPOLA, CUPOLA.map((_, i) => (i === 0 ? GABLE : ROOF))));
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
    return b.mesh();
}

/** Every template, in the order of TEMPLATES. */
export const MESHES: Record<Template, Mesh> = {
    gable: gable(), flat: flat(), spire: church('spire'), onion: church('onion'), domes: church('domes'), terrace: terrace(), cones: cones(), granary: granary(),
};
