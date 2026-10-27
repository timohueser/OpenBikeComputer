// The furniture of outdoor places, in metres with the base at the origin, z up and the front toward −y:
// generic shapes that fit any region.
import { rgb } from '../structures/mesh';
import { Model, roof, type Vec } from './flat';

const WOOD = rgb('#8a6440'), PALE_WOOD = rgb('#a5815a'), DARK_WOOD = rgb('#5e4630'), METAL = rgb('#4b5157');
const STONE = rgb('#9a958c'), DARK_STONE = rgb('#77726a'), ASH = rgb('#3a3430'), WATER = rgb('#5d8fb0'), JET = rgb('#cfe6f2');
const FLAME = rgb('#ff7a1c'), CORE = rgb('#ffd04a'), POST = rgb('#2f7fc1'), PUMP = rgb('#c8402a'), ROOF = rgb('#7a5136');
// As the logo, brand/signpost.svg.
const OLIVE = rgb('#5c5a2e'), SIGN = rgb('#f4a81d');
const POLE = rgb('#4a4f54'), LAMP = rgb('#fff1c4');
const BAG = rgb('#f08a24'), CROSS = rgb('#2fa84f'), HOSPITAL = rgb('#1f5fae'), WHITE = rgb('#f7f7f2');

/** A park bench: a seat, a back and two pairs of legs. */
export function bench(): Model {
    const m = new Model().box([0, 0, 0.45], [1.6, 0.42, 0.05], WOOD).box([0, 0.2, 0.74], [1.6, 0.04, 0.32], WOOD);
    for (const x of [-0.68, 0.68]) m.box([x, -0.16, 0.21], [0.06, 0.06, 0.42], METAL).box([x, 0.2, 0.46], [0.06, 0.06, 0.92], METAL);
    return m;
}

/** A picnic table with a bench along each side on A-frames. */
export function picnicTable(): Model {
    const m = new Model().box([0, 0, 0.75], [1.8, 0.75, 0.05], PALE_WOOD);
    for (const y of [-0.65, 0.65]) m.box([0, y, 0.45], [1.8, 0.28, 0.05], PALE_WOOD);
    for (const x of [-0.7, 0.7]) {
        m.beam([x, -0.8, 0], [x, 0, 0.73], 0.08, WOOD).beam([x, 0.8, 0], [x, 0, 0.73], 0.08, WOOD);
        m.box([x, 0, 0.4], [0.07, 1.6, 0.06], WOOD);
    }
    return m;
}

/** A fireplace: a ring of stones about ash and a tipi of logs, with a flame that burns at night. */
export function firepit(): Model {
    const m = new Model(), stones = 9;
    m.poly(Array.from({ length: 8 }, (_, k): Vec => [0.5 * Math.cos(k * Math.PI / 4), 0.5 * Math.sin(k * Math.PI / 4), 0.03]), ASH);
    for (let k = 0; k < stones; k++) {
        const a = (2 * Math.PI * k) / stones;
        m.add(new Model().box([0, 0, 0.09], [0.22, 0.3, 0.18], k % 2 ? STONE : DARK_STONE).place(a, [0.62 * Math.cos(a), 0.62 * Math.sin(a), 0]));
    }
    for (let k = 0; k < 3; k++) {
        const a = (2 * Math.PI * k) / 3 + 0.4;
        m.beam([0.32 * Math.cos(a), 0.32 * Math.sin(a), 0.04], [0, 0, 0.42], 0.08, DARK_WOOD);
    }
    return fire(m, 0.05, 1);
}

/** A barbecue: a stone hearth at waist height behind a low wall, with a grate over a fire that burns at night. */
export function bbq(): Model {
    const m = new Model().box([0, 0, 0.35], [1.1, 0.8, 0.7], STONE).box([0, 0.33, 0.88], [1.1, 0.14, 0.36], DARK_STONE);
    for (const x of [-0.5, 0.5]) m.box([x, -0.03, 0.82], [0.1, 0.6, 0.24], DARK_STONE);
    m.box([0, -0.03, 0.71], [0.88, 0.58, 0.02], ASH);
    for (let k = 0; k < 6; k++) m.beam([-0.42, -0.27 + 0.1 * k, 0.86], [0.42, -0.27 + 0.1 * k, 0.86], 0.02, METAL);
    return fire(m, 0.72, 0.45);
}

/** Two crossed tongues of fire and a core from `z`, `scale` times their size in a fireplace, that rise and glow at night. */
function fire(m: Model, z: number, scale: number): Model {
    const first = m.triangles;
    for (const [a, w, h, colour] of [[0, 0.26, 0.8, FLAME], [Math.PI / 2, 0.26, 0.7, FLAME], [Math.PI / 4, 0.14, 0.45, CORE]] as const) {
        const c = Math.cos(a) * w * scale, s = Math.sin(a) * w * scale;
        m.tri([-c, -s, z], [c, s, z], [0, 0, z + h * scale], colour);
    }
    return m.glowing(first, true);
}

/** A guidepost as the logo draws it: an olive post with two arrow plates that point either way along +x. */
export function guidepost(): Model {
    const m = new Model().box([0, 0, 1.25], [0.12, 0.12, 2.5], OLIVE);
    for (const [z, way, tilt] of [[2.2, -1, 0.07], [1.85, 1, -0.07]]) {
        const plate = new Model().prism([[0.06, -0.1], [0.06, 0.1], [0.56, 0.1], [0.7, 0], [0.56, -0.1]], -0.02, 0.02, SIGN);
        m.add(plate.map(([x, y, h]) => [way * x, y, z + h + tilt * x]));
    }
    return m;
}

/** A street lamp: a pole whose arm reaches toward −y to a lantern under a cap, which glows at night. */
export function streetLamp(): Model {
    const m = new Model().beam([0, 0, 0], [0, 0, 6], 0.14, POLE, { sides: 6 }).beam([0, 0, 5.85], [0, -0.9, 6.05], 0.07, POLE);
    m.box([0, -1.05, 6.08], [0.3, 0.56, 0.05], POLE);
    const glass = m.triangles;
    return m.box([0, -1.05, 5.97], [0.24, 0.48, 0.17], LAMP).glowing(glass);
}

/** A drinking fountain: a stone trough of water with a pillar and its spout. */
export function drinkingWater(): Model {
    const m = new Model().box([0, 0, 0.3], [1.1, 0.5, 0.6], [STONE, STONE, STONE, STONE, WATER, STONE]);
    m.box([0, 0.34, 0.6], [0.28, 0.22, 1.2], DARK_STONE).box([0, 0.34, 1.23], [0.34, 0.28, 0.06], STONE);
    // The spout and the jet of water that falls from it into the trough.
    return m.beam([0, 0.24, 1.0], [0, -0.08, 0.95], 0.07, METAL).beam([0, -0.09, 0.93], [0, -0.1, 0.6], 0.04, JET);
}

/** An open wooden shelter: four posts, a back wall, a gable roof and a bench inside. */
export function shelter(): Model {
    const m = new Model().box([1.15, 0, 1.05], [0.08, 3, 2.1], WOOD).box([0.85, 0, 0.45], [0.4, 2.8, 0.05], PALE_WOOD);
    for (const x of [-1.15, 1.15]) for (const y of [-1.45, 1.45]) m.box([x, y, 1.05], [0.12, 0.12, 2.1], DARK_WOOD);
    return roof(m, [0, 0, 2.1], 2.4, 3, 30, 0.3, ROOF, WOOD);
}

/** A bike repair station: a post with two arms to hang a bike on, and a pump. */
export function bikeRepair(): Model {
    const m = new Model().box([0, 0, 0.02], [0.8, 0.6, 0.04], STONE).box([0, 0, 0.72], [0.12, 0.12, 1.4], POST);
    for (const x of [-1, 1]) m.beam([0.04 * x, 0, 1.32], [0.34 * x, 0, 1.48], 0.04, METAL);
    return m.beam([0.28, -0.2, 0.04], [0.28, -0.2, 0.95], 0.13, PUMP, { sides: 6, capped: true });
}

// The signs of a building's use: a symbol on a bracket across a wall in the x–z plane, so that the street
// toward −y sees it from both ways, at the height of the ground floor, which every building has. It glows at night.

/** The bracket from the wall, and the middle of the symbol that it holds. */
function bracket(): { m: Model; at: Vec } {
    return { m: new Model().beam([0, 0, 2.8], [0, -0.45, 2.8], 0.05, METAL), at: [0, -0.78, 2.8] };
}

/** A shop: a shopping bag with its handle. */
export function shopSign(): Model {
    const { m, at: [x, y, z] } = bracket(), symbol = m.triangles;
    m.box([x, y, z - 0.04], [0.08, 0.44, 0.38], BAG);
    for (const [a, b] of [[[y + 0.12, z + 0.15], [y + 0.1, z + 0.29]], [[y + 0.1, z + 0.29], [y - 0.1, z + 0.29]], [[y - 0.1, z + 0.29], [y - 0.12, z + 0.15]]]) {
        m.beam([x, a[0], a[1]], [x, b[0], b[1]], 0.035, BAG);
    }
    return m.glowing(symbol);
}

/** A pharmacy: a green cross. */
export function pharmacySign(): Model {
    const { m, at } = bracket(), symbol = m.triangles;
    return m.box(at, [0.08, 0.18, 0.56], CROSS).box(at, [0.08, 0.56, 0.18], CROSS).glowing(symbol);
}

/** A hospital: a white H on a blue square, as the road sign shows it, on both faces. */
export function hospitalSign(): Model {
    const { m, at: [x, y, z] } = bracket(), symbol = m.triangles;
    m.box([x, y, z], [0.06, 0.56, 0.56], HOSPITAL);
    for (const face of [-0.04, 0.04]) {
        for (const dy of [-0.14, 0.14]) m.box([x + face, y + dy, z], [0.02, 0.08, 0.38], WHITE);
        m.box([x + face, y, z], [0.02, 0.22, 0.08], WHITE);
    }
    return m.glowing(symbol);
}
