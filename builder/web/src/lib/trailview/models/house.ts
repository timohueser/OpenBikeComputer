// The house templates: the faces of the prototype's buildings (src/models/buildings.ts) as vertices
// whose coordinates name slots, not numbers. houses.wgsl fills the slots of each house from its
// footprint, its style and its ground, so one template draws every house of its shape.

/** The templates, as `T_*` in houses.wgsl. */
export const TEMPLATES = ['gable', 'flat', 'spire', 'onion'] as const;
export type Template = (typeof TEMPLATES)[number];

/** An onion dome, its lantern and its spike, bottom up: radius and height as shares of the tower's half side. */
const ONION = [[0.55, 0], [0.8, 0.35], [0.8, 0.7], [0.55, 1.05], [0.22, 1.35], [0.1, 1.6], [0.1, 1.9], [0.2, 2.05], [0.06, 2.3], [0, 2.9]];

/** Faces, as `M_*` in houses.wgsl: the fragment shader paints each by its material. */
export const WALL = 0, GABLE = 1, ROOF = 2, EDGE = 3, SOFFIT = 4, CHIMNEY = 5, CAP = 6, BALCONY = 7, TOWER = 8, FLAT = 9;

/**
 * A vertex: slots of x, y and z, each with a sign, offsets in centimetres, the material and the wall.
 * An onion vertex lies on ring `ring` of the dome at the eighth `turn` of a full turn about the tower.
 */
export interface Corner { x: number; y: number; z: number; sx?: number; sy?: number; dz?: number; ring?: number; turn?: number }

/**
 * Two words per vertex, as `template_vertex` in houses.wgsl reads them. Word 0: x slot (4 bits), y slot
 * (4), z slot (4), the signs of x and y (2), onion (1), material (4) at bit 15, wall (3) at bit 19, and
 * turn (3) at bit 26. Word 1: the offset of z in centimetres, signed 16 bits; on the onion dome, its
 * ring's radius in hundredths (8 bits) and height in fiftieths (8) of the tower's half side.
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

    /** The end of the simple level of detail. */
    mark(): void {
        this.simple = this.words.length / VERTEX_WORDS;
    }

    mesh(): Mesh {
        return { words: new Uint32Array(this.words), simple: this.simple, full: this.words.length / VERTEX_WORDS };
    }

    private vertex({ x, y, z, sx = 1, sy = 1, dz = 0, ring, turn = 0 }: Corner, material: number, wall: number): void {
        const onion = ring !== undefined;
        const [radius, height] = ONION[ring ?? 0];
        this.words.push((x | (y << 4) | (z << 8) | (+(sx < 0) << 12) | (+(sy < 0) << 13) | (+onion << 14) | (material << 15) | (wall << 19) | (turn << 26)) >>> 0,
            ((dz & 0xffff) | (onion ? (Math.round(radius * 100) << 16) | (Math.round(height * 50) << 24) : 0)) >>> 0);
    }
}

/** The four walls from the bottom to slot `top`, counter-clockwise from above, the first along −y: x slot `x`, y slot `y`. */
function walls(b: Builder, x: number, y: number, top: number, material = WALL): void {
    const corners: [number, number][] = [[-1, -1], [1, -1], [1, 1], [-1, 1]];
    corners.forEach(([ax, ay], w) => {
        const [bx, by] = corners[(w + 1) % 4];
        b.poly([{ x, y, z: Z.BOTTOM, sx: ax, sy: ay }, { x, y, z: Z.BOTTOM, sx: bx, sy: by }, { x, y, z: top, sx: bx, sy: by }, { x, y, z: top, sx: ax, sy: ay }], material, w);
    });
}

/** A slot with its sign. */
type Bound = [number, number];

/** A box from x0 to x1, y0 to y1 (x0 < x1, y0 < y1) and z slots z0 to z1, with its top in `top` and a floor in `floor`. */
function box(b: Builder, [x0, x1]: [Bound, Bound], [y0, y1]: [Bound, Bound], [z0, z1]: [number, number], side: number, top: number, floor?: number): void {
    const ring = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(([[x, sx], [y, sy]]) => ({ x, sx, y, sy }));
    const at = (c: Omit<Corner, 'z'>, z: number) => ({ ...c, z });
    if (floor !== undefined) b.poly(ring.map(c => at(c, z0)).reverse(), floor);
    ring.forEach((c, i) => {
        const d = ring[(i + 1) % 4];
        b.poly([at(c, z0), at(d, z0), at(d, z1), at(c, z1)], side);
    });
    b.poly(ring.map(c => at(c, z1)), top);
}

/** The z slots of every template, as `Z_*` in houses.wgsl: 0 is the bottom of the walls, under the ground. */
export const Z = { BOTTOM: 0, WALL: 1 } as const;

/**
 * Slots of the gable template, as `slots_of` in houses.wgsl fills them. x: L/2, the roof's end, the ridge's
 * end, then the two sides of each chimney and each balcony's half-length, zero without them. y: W/2,
 * the eaves, the hip's foot, the gable's top, a chimney's half depth, a balcony's outer side. z: the
 * eaves, the ridge, the hip, the gable's top, the chimneys' foot and tops, and the balconies' floors
 * and tops: an absent chimney or balcony has no height, so all its faces have no area.
 */
const G = {
    X: { HALF: 1, END: 2, RIDGE: 3, C1A: 4, C1B: 5, C2A: 6, C2B: 7, B1: 8, B2: 9, ZERO: 0 },
    Y: { HALF: 1, EAVE: 2, HIP: 3, TOP: 4, CHIMNEY: 5, BALCONY: 6, ZERO: 0 },
    Z: { EAVE: 2, RIDGE: 3, HIP: 4, TOP: 5, C0: 6, C1: 7, C2: 8, B1A: 9, B1B: 10, B2A: 11, B2B: 12 },
};

function gable(): Mesh {
    const b = new Builder(), { X, Y, Z: GZ } = G;
    const roof = (tops: boolean) => {
        for (const s of [1, -1]) {
            // A side of the roof, mirrored by s through the ridge and the middle.
            const under: Corner[] = [
                { x: X.END, y: Y.EAVE, z: GZ.EAVE, sx: -s, sy: -s }, { x: X.END, y: Y.EAVE, z: GZ.EAVE, sx: s, sy: -s },
                { x: X.END, y: Y.HIP, z: GZ.HIP, sx: s, sy: -s }, { x: X.RIDGE, y: Y.ZERO, z: GZ.RIDGE, sx: s },
                { x: X.RIDGE, y: Y.ZERO, z: GZ.RIDGE, sx: -s }, { x: X.END, y: Y.HIP, z: GZ.HIP, sx: -s, sy: -s },
            ];
            b.slab(under, tops);
            b.slab([{ x: X.END, y: Y.HIP, z: GZ.HIP, sx: s, sy: -s }, { x: X.END, y: Y.HIP, z: GZ.HIP, sx: s, sy: s }, { x: X.RIDGE, y: Y.ZERO, z: GZ.RIDGE, sx: s }], tops);
        }
    };
    walls(b, X.HALF, Y.HALF, Z.WALL);
    for (const s of [1, -1]) {
        b.poly([{ x: X.HALF, y: Y.HALF, z: Z.WALL, sx: s, sy: -s }, { x: X.HALF, y: Y.HALF, z: Z.WALL, sx: s, sy: s }, { x: X.HALF, y: Y.TOP, z: GZ.TOP, sx: s, sy: s }, { x: X.HALF, y: Y.TOP, z: GZ.TOP, sx: s, sy: -s }], GABLE);
    }
    roof(true);
    b.mark();
    roof(false);
    const chimney: [Bound, Bound] = [[Y.CHIMNEY, -1], [Y.CHIMNEY, 1]], balcony: [Bound, Bound] = [[Y.BALCONY, -1], [Y.HALF, -1]];
    box(b, [[X.C1A, 1], [X.C1B, 1]], chimney, [GZ.C0, GZ.C1], CHIMNEY, CAP);
    box(b, [[X.C2B, -1], [X.C2A, -1]], chimney, [GZ.C0, GZ.C2], CHIMNEY, CAP);
    box(b, [[X.B1, -1], [X.B1, 1]], balcony, [GZ.B1A, GZ.B1B], BALCONY, BALCONY, SOFFIT);
    box(b, [[X.B2, -1], [X.B2, 1]], balcony, [GZ.B2A, GZ.B2B], BALCONY, BALCONY, SOFFIT);
    return b.mesh();
}

/** Slots of the flat template, as `slots_of` in houses.wgsl fills them: the walls' half sides, and the roof's 20 cm past them. */
function flat(): Mesh {
    const b = new Builder();
    walls(b, 1, 1, Z.WALL);
    const roof: Corner[] = [[-1, -1], [1, -1], [1, 1], [-1, 1]].map(([sx, sy]) => ({ x: 2, y: 2, z: Z.WALL, sx, sy }));
    b.slab(roof, true, FLAT);
    b.mark();
    b.slab(roof, false, FLAT);
    return b.mesh();
}

/**
 * Slots of the church templates, as `slots_of` in houses.wgsl fills them, with x signed: the nave's west and
 * east walls and roof ends, the tower's west side, its middle, and the spire's eaves. y: W/2, the
 * eaves, the tower's half side and the spire's eaves. z: the eaves, the ridge, the tower's top, the
 * spire's eaves and its apex.
 */
const C = {
    X: { NAVE_W: 1, NAVE_E: 2, END_W: 3, END_E: 4, TOWER_W: 5, TOWER: 6, SPIRE_W: 7, SPIRE_E: 8 },
    Y: { HALF: 1, EAVE: 2, TOWER: 3, SPIRE: 4, ZERO: 0 },
    Z: { EAVE: 2, RIDGE: 3, TOP: 4, SPIRE: 5, APEX: 6 },
};

function church(onion: boolean): Mesh {
    const b = new Builder(), { X, Y, Z: CZ } = C;
    const nave = (tops: boolean) => {
        b.slab([{ x: X.END_W, y: Y.EAVE, z: CZ.EAVE, sy: -1 }, { x: X.END_E, y: Y.EAVE, z: CZ.EAVE, sy: -1 }, { x: X.END_E, y: Y.ZERO, z: CZ.RIDGE }, { x: X.END_W, y: Y.ZERO, z: CZ.RIDGE }], tops);
        b.slab([{ x: X.END_E, y: Y.EAVE, z: CZ.EAVE }, { x: X.END_W, y: Y.EAVE, z: CZ.EAVE }, { x: X.END_W, y: Y.ZERO, z: CZ.RIDGE }, { x: X.END_E, y: Y.ZERO, z: CZ.RIDGE }], tops);
    };
    const spire = (tops: boolean) => {
        if (onion) {
            b.slab([[X.SPIRE_W, -1], [X.SPIRE_E, -1], [X.SPIRE_E, 1], [X.SPIRE_W, 1]].map(([x, sy]) => ({ x, y: Y.SPIRE, z: CZ.TOP, sy })), tops);
            if (!tops) return;
            for (let k = 0; k + 1 < ONION.length; k++) for (let i = 0; i < 8; i++) {
                const at = (ring: number, turn: number): Corner => ({ x: X.TOWER, y: Y.ZERO, z: CZ.TOP, ring, turn: turn % 8 });
                b.poly([at(k, i), at(k, i + 1), at(k + 1, i + 1), at(k + 1, i)], ROOF);
            }
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
export const MESHES: Record<Template, Mesh> = { gable: gable(), flat: flat(), spire: church(false), onion: church(true) };
