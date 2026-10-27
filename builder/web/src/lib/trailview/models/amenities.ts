// The furniture of outdoor places, in metres with the base at the origin, z up and the front toward −y:
// generic shapes that fit any region.
import { rgb } from '../structures/mesh';
import { Model, roof, type Vec } from './flat';

const WOOD = rgb('#8a6440'), PALE_WOOD = rgb('#a5815a'), DARK_WOOD = rgb('#5e4630'), METAL = rgb('#4b5157');
const STONE = rgb('#9a958c'), DARK_STONE = rgb('#77726a'), ASH = rgb('#3a3430'), WATER = rgb('#5d8fb0'), JET = rgb('#cfe6f2');
const FLAME = rgb('#ff7a1c'), CORE = rgb('#ffd04a'), POST = rgb('#2f7fc1'), PUMP = rgb('#c8402a'), ROOF = rgb('#7a5136');

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
    // Two crossed tongues of fire and a core, each seen from both sides.
    const first = m.triangles;
    for (const [a, w, h, colour] of [[0, 0.26, 0.85, FLAME], [Math.PI / 2, 0.26, 0.75, FLAME], [Math.PI / 4, 0.14, 0.5, CORE]] as const) {
        const c = Math.cos(a), s = Math.sin(a), l: Vec = [-w * c, -w * s, 0.05], r: Vec = [w * c, w * s, 0.05], top: Vec = [0, 0, h];
        m.tri(l, r, top, colour).tri(r, l, top, colour);
    }
    return m.glowing(first);
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
