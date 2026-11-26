// The furniture of outdoor places, in metres with the base at the origin, z up and the front toward −y:
// generic shapes that fit any region.
import { rgb } from '../structures/mesh';
import { MOUTH_BACK_M, MOUTH_FRONT_M, MOUTH_HALF_M, MOUTH_WATER_M } from '../terrain/culverts';
import { Model, shade, type Vec } from './flat';
import { alongY, bands, Sculpt, through, type Paint } from './sculpt';

const WOOD = rgb('#8a6440'), PALE_WOOD = rgb('#a5815a'), WOOD_LIGHT = rgb('#b08d66'), DARK_WOOD = rgb('#5e4630'), METAL = rgb('#4b5157');
const STONE = rgb('#9a958c'), DARK_STONE = rgb('#77726a'), PALE_STONE = rgb('#b3aea3'), CHARRED = rgb('#2b2622'), ASH = rgb('#3a3430'), WATER = rgb('#5d8fb0'), JET = rgb('#cfe6f2');
const FLAME = rgb('#ff7a1c'), CORE = rgb('#ffd04a'), EMBER = rgb('#ee4b1a'), POST = rgb('#2f7fc1'), PUMP = rgb('#c8402a'), ROOF = rgb('#7a5136'), ROOF_LIGHT = rgb('#8a5f40'), BRASS = rgb('#b28a3a');
// As the logo, brand/signpost.svg.
const OLIVE = rgb('#5c5a2e'), DARK_OLIVE = rgb('#45441f'), SIGN = rgb('#f4a81d');
const POLE = rgb('#5b636b'), SLEEVE = rgb('#444b52'), HEAD = rgb('#3a4046'), LAMP = rgb('#fff1c4');
const IRON = rgb('#2f3d36'), IRON_LIGHT = rgb('#3b4a42');
const CONCRETE = rgb('#9b988e'), COPING = rgb('#b4b1a7'), DARK = rgb('#15181a');

/**
 * A bench: a seat of three slats and a back of two, which leans back, on two iron frames with arm rests. It keeps the
 * footprint of a seat 1.6 m wide: its place beside a way reads the model's reach.
 */
export function bench(): Model {
    const m = new Model();
    [-0.15, -0.01, 0.13].forEach((y, k) => m.box([0, y, 0.445], [1.6, 0.12, 0.05], k % 2 ? PALE_WOOD : WOOD));
    // The back's slats follow the lean of the frames' back legs.
    for (const [z, wood] of [[0.66, PALE_WOOD], [0.82, WOOD]]) m.add(new Model().box([0, 0, 0], [1.6, 0.035, 0.13], wood).map(([x, y, h]) => [x, 0.155 + 0.075 * (z + h - 0.42) + y, z + h]));
    for (const x of [-0.7, 0.7]) {
        // A front leg up to the arm rest, a back leg that leans back to the top of the back, a rail under the seat and the arm rest.
        m.beam([x, -0.17, 0], [x, -0.17, 0.64], 0.065, IRON, { capped: true }).beam([x, 0.13, 0], [x, 0.1975, 0.9], 0.065, IRON, { capped: true });
        m.box([x, -0.01, 0.39], [0.065, 0.36, 0.06], IRON).box([x, -0.02, 0.66], [0.085, 0.42, 0.04], IRON_LIGHT);
    }
    return m;
}

/**
 * A picnic table of planks: a top of five and a bench of two along each side, on two A-frames whose legs stand outside
 * the benches, with a beam under the benches, a cleat under the top and a brace from each beam to the top's middle.
 */
export function picnicTable(): Model {
    const m = new Model(), plank = (y: number, z: number, k: number) => m.box([0, y, z], [1.8, 0.14, 0.05], k % 2 ? WOOD_LIGHT : PALE_WOOD);
    [-0.304, -0.152, 0, 0.152, 0.304].forEach((y, k) => plank(y, 0.75, k));
    for (const side of [-1, 1]) [0.578, 0.722].forEach((y, k) => plank(side * y, 0.45, k + 1));
    for (const x of [-0.62, 0.62]) {
        for (const side of [-1, 1]) m.beam([x, side * 0.796, 0], [x, side * 0.22, 0.725], 0.08, WOOD, { capped: true });
        m.box([x, 0, 0.38], [0.07, 1.62, 0.09], WOOD).box([x, 0, 0.695], [0.07, 0.72, 0.06], WOOD);
        m.beam([x, 0, 0.4], [0.36 * x, 0, 0.71], 0.06, WOOD, { capped: true });
    }
    return m;
}

/** A steady share from 0 to 1 for part k of a model and its corner j: the unevenness of a stone or a flame. */
const uneven = (k: number, j: number) => ((k * 37 + j * 17 + 5) % 11) / 10;

/** A fieldstone about the origin: an uneven block of five sides, `size` across and `high`, that narrows to a small top. */
export function fieldstone(k: number, size: number, high: number, colour: number): Model {
    const foot = Array.from({ length: 5 }, (_, j): [number, number] => {
        const a = (2 * Math.PI * (j + 0.3 * uneven(k, j))) / 5, r = (size / 2) * (0.85 + 0.3 * uneven(k + 3, j));
        return [r * Math.cos(a), r * Math.sin(a)];
    });
    return new Model().loft(foot, [[0, 1], [0.6 * high, 0.95], [high, 0.55]], band => (band ? shade(colour, 1.08) : colour), colour, shade(colour, 1.12));
}

/** A fireplace: a ring of fieldstones about ash and a tipi of logs, with a flame that burns at night. */
export function firepit(): Model {
    const m = new Model(), stones = 9;
    m.poly(Array.from({ length: 8 }, (_, k): Vec => [0.5 * Math.cos(k * Math.PI / 4), 0.5 * Math.sin(k * Math.PI / 4), 0.03]), ASH);
    for (let k = 0; k < stones; k++) {
        const a = (2 * Math.PI * k) / stones, r = 0.562 + 0.03 * uneven(k, 1);
        m.add(fieldstone(k, 0.26 + 0.06 * uneven(k, 2), 0.16 + 0.07 * uneven(k, 4), [STONE, DARK_STONE, PALE_STONE][k % 3]).place(a + uneven(k, 6), [r * Math.cos(a), r * Math.sin(a), 0]));
    }
    for (let k = 0; k < 4; k++) {
        const a = (2 * Math.PI * k) / 4 + 0.4;
        m.beam([0.34 * Math.cos(a), 0.34 * Math.sin(a), 0.04], [-0.03 * Math.cos(a), -0.03 * Math.sin(a), 0.46], 0.085, k % 2 ? DARK_WOOD : CHARRED, { sides: 5, capped: true });
    }
    return fire(m, 0.05, 1);
}

/**
 * A barbecue: a hearth of stone at waist height under a coping, with a store of logs in its front, walls on three sides
 * of its coals and a grate over a fire that burns at night.
 */
export function bbq(): Model {
    const m = new Model().box([0, 0.005, 0.31], [1.04, 0.73, 0.62], STONE).box([0, 0, 0.66], [1.1, 0.8, 0.08], PALE_STONE);
    // The log store: a dark opening with the ends of five logs.
    m.box([0, -0.365, 0.3], [0.52, 0.02, 0.36], CHARRED);
    for (const [x, z] of [[-0.13, 0.2], [0, 0.2], [0.13, 0.2], [-0.065, 0.31], [0.065, 0.31]]) {
        m.poly(Array.from({ length: 6 }, (_, k): Vec => [x + 0.06 * Math.cos((k * Math.PI) / 3), -0.378, z + 0.06 * Math.sin((k * Math.PI) / 3)]).reverse(), PALE_WOOD);
    }
    m.box([0, 0.33, 0.9], [1.1, 0.14, 0.4], DARK_STONE).box([0, 0.33, 1.12], [1.1, 0.14, 0.04], PALE_STONE);
    for (const x of [-0.5, 0.5]) m.box([x, -0.03, 0.83], [0.1, 0.58, 0.26], DARK_STONE);
    m.box([0, -0.03, 0.715], [0.9, 0.58, 0.03], ASH);
    // The grate: two rails and the bars across them.
    for (const y of [-0.3, 0.24]) m.box([0, y, 0.9], [0.9, 0.025, 0.025], METAL);
    for (let k = 0; k < 8; k++) m.beam([-0.4 + 0.114 * k, -0.3, 0.9], [-0.4 + 0.114 * k, 0.24, 0.9], 0.022, METAL, { sides: 3 });
    return fire(m, 0.73, 0.45);
}

/**
 * A fire from `z`, `scale` times its size in a fireplace: three tongues of five sides, bright at the foot and red at
 * the tip, which rise and glow at night.
 */
function fire(m: Model, z: number, scale: number): Model {
    const first = m.triangles;
    for (const [k, x, y, r, h] of [[0, 0, 0, 0.17, 0.82], [1, 0.16, 0.06, 0.11, 0.55], [2, -0.14, -0.1, 0.1, 0.47]]) {
        const foot = Array.from({ length: 5 }, (_, j): [number, number] => [r * scale * Math.cos((2 * Math.PI * j) / 5 + k), r * scale * Math.sin((2 * Math.PI * j) / 5 + k)]);
        const lean = 0.08 * (uneven(k, 2) - 0.5);
        m.add(new Model().loft(foot, [[0, 0.7], [0.28 * h * scale, 1.05], [0.62 * h * scale, 0.6], [h * scale, 0.04]], band => [CORE, FLAME, EMBER][band], CORE, EMBER).map(([px, py, pz]) => [px + x * scale + lean * pz, py + y * scale, z + pz]));
    }
    return m.glowing(first, true);
}

/** A guidepost as the logo draws it: an olive post under a small ridge with two arrow plates that point either way along +x. */
export function guidepost(): Model {
    const m = new Model().box([0, 0, 1.21], [0.12, 0.12, 2.42], OLIVE).prism([[-0.065, 2.42], [0.065, 2.42], [0, 2.5]], -0.06, 0.06, DARK_OLIVE);
    for (const [z, way, tilt] of [[2.2, -1, 0.07], [1.85, 1, -0.07]]) {
        const plate = new Model().prism([[0.06, -0.1], [0.06, 0.1], [0.56, 0.1], [0.7, 0], [0.56, -0.1]], -0.02, 0.02, SIGN);
        m.add(plate.map(([x, y, h]) => [way * x, way * y, z + h + tilt * x]));
    }
    return m;
}

/** The middle of the bottom of streetLamp's glass, where its light comes from, in metres from its foot. */
export const LANTERN: Vec = [0, -1.05, 5.885];

/**
 * A street lamp: a tapered pole in a sleeve that bends over toward −y into an arm, and a head whose glass glows at
 * night. The far tier keeps the outline with fewer rings and sides.
 */
export function streetLamp(tier = 0): Model {
    const s = new Sculpt(tier), [, y, z] = LANTERN, middle = z + 0.115;
    // The pole's path [y, z] and its half-width there.
    const path = tier ? [[0, 0, 0.085], [0, 5.4, 0.056], [-0.18, 5.82, 0.05], [-0.7, 6, 0.046]]
        : [[0, 0, 0.105], [0, 0.9, 0.105], [0, 1.02, 0.07], [0, 5.4, 0.052], [-0.05, 5.63, 0.05], [-0.18, 5.82, 0.048], [-0.37, 5.95, 0.046], [-0.6, 6, 0.044], [-0.7, 6, 0.043]];
    s.skin(through(path.map(([py, pz]): Vec => [0, py, pz]), path.map(([, , w]) => [w, w, w])), 6, tier ? POLE : bands(SLEEVE, SLEEVE, SLEEVE, POLE));
    s.fine(() => s.box([0, 0, 0.02], [0.34, 0.34, 0.04], SLEEVE));
    s.skin(alongY([[-0.64, middle, 0.09, 0.05, 0.04], [-0.8, middle, 0.19, 0.08, 0.065], [-1.3, middle, 0.19, 0.07, 0.065], [-1.43, middle, 0.09, 0.035, 0.045]], 3.5), 8, HEAD);
    const glass = s.model.triangles;
    return s.model.prism([[-0.15, z + 0.07], [0.15, z + 0.07], [0.11, z], [-0.11, z]], y - 0.24, y + 0.24, LAMP).glowing(glass);
}

/** The middle of streetLantern's glass, where its light comes from, in metres from its foot. */
export const LANTERN_GLASS: Vec = [0, 0, 3.42];

/**
 * A street lantern of an old street: a cast-iron post on a plinth with a ladder bar, and a four-sided lantern under a
 * roof, whose glass glows at night. The far tier keeps the outline.
 */
export function streetLantern(tier = 0): Model {
    const s = new Sculpt(tier), [, , z] = LANTERN_GLASS, low = z - 0.2, high = z + 0.2;
    /** A round part about the post's axis from rows of [z, radius]. */
    const turned = (rows: number[][], sides: number, paint: Paint | number, e = 2) => s.skin(rows.map(([at, r]) => ({ at: [0, 0, at] as Vec, w: r, up: r, down: r, pitch: 90, e })), sides, paint);
    turned(tier ? [[0, 0.14], [0.6, 0.1], [0.7, 0.07], [low - 0.1, 0.045]]
        : [[0, 0.15], [0.46, 0.15], [0.52, 0.11], [0.64, 0.1], [0.72, 0.07], [2.6, 0.048], [2.66, 0.075], [2.72, 0.048], [low - 0.1, 0.045]], 6, IRON);
    // The lantern: a cup, the glass that widens upward, a roof with a wide eave and a finial.
    turned([[low - 0.1, 0.045], [low, 0.125]], 4, IRON);
    const glass = s.model.triangles;
    turned([[low, 0.12], [high, 0.19]], 4, LAMP);
    s.model.glowing(glass);
    turned([[high - 0.01, 0.27], [high + 0.04, 0.25], [high + 0.19, 0.09], [high + 0.25, 0.07]], 4, IRON_LIGHT);
    s.fine(() => {
        turned([[high + 0.25, 0.028], [high + 0.38, 0.028]], 4, IRON);
        // The bars at the glass's corners, and the bar that a ladder leans on.
        // A skin of four sides has its corners 0.886 of its radius along x and y.
        for (const [x, y] of [[1, 1], [-1, 1], [-1, -1], [1, -1]]) s.chain([[x * 0.108, y * 0.108, low], [x * 0.17, y * 0.17, high]], [0.03, 0.03], IRON, 3);
        s.chain([[-0.3, 0, low - 0.22], [0.3, 0, low - 0.22]], [0.035, 0.035], IRON, 4);
    });
    return s.model;
}

/**
 * The mouth of a culvert: a low concrete headwall behind the origin, as wide as a stream's ribbon and 0.25 m on each
 * side, battered and capped so that from above it is no block, with a ring of 0.6 m round its dark bore and a wing wall
 * along each side of the water. A ribbon ends at the origin and draws up to 0.45 m nearer the eye: so it stays off the
 * ring, and the wall's foot is 0.5 m thick to hide it from behind. The wall's foot lies below a bank.
 */
export function culvertMouth(): Model {
    /** A profile [y, z] drawn along x. */
    const across = (profile: [number, number][], x0: number, x1: number, colour: number) => new Model().prism(profile, -x1, -x0, colour).map(([x, y, z]) => [-y, x, z]);
    const inner = MOUTH_WATER_M, outer = MOUTH_HALF_M - 0.1, wing: [number, number][] = [[-MOUTH_FRONT_M, -0.5], [0, -0.5], [0, 0.45], [-MOUTH_FRONT_M, 0]];
    const m = across([[0, -0.7], [MOUTH_BACK_M, -0.7], [0.3, 0.58], [0, 0.58]], -outer, outer, CONCRETE);
    m.add(across([[-0.04, 0.58], [0.38, 0.58], [0.3, 0.64], [0, 0.64]], -MOUTH_HALF_M, MOUTH_HALF_M, COPING));
    m.add(across(wing, inner, outer, CONCRETE)).add(across(wing, -outer, -inner, CONCRETE));
    /** A circle about the bore's middle, clockwise seen from its front, at y. */
    const circle = (r: number, y: number) => Array.from({ length: 10 }, (_, k): Vec => [r * Math.cos((2 * Math.PI * k) / 10), y, 0.3 + r * Math.sin((2 * Math.PI * k) / 10)]);
    const back = circle(0.3, 0), front = circle(0.3, -0.06);
    for (let k = 0; k < 10; k++) m.poly([back[k], back[(k + 1) % 10], front[(k + 1) % 10], front[k]], COPING);
    return m.poly(front, COPING).poly(circle(0.23, -0.07), DARK);
}

/**
 * A drinking fountain: a stone trough of water on a plinth, and behind it a pillar under a cap, with a brass spout and
 * the jet of water that falls from it into the trough.
 */
export function drinkingWater(): Model {
    const m = new Model().box([0, 0, 0.04], [1, 0.42, 0.08], DARK_STONE).box([0, 0, 0.11], [0.98, 0.38, 0.06], DARK_STONE);
    // The trough's four walls about the water.
    for (const y of [-0.215, 0.215]) m.box([0, y, 0.34], [1.1, 0.07, 0.52], STONE);
    for (const x of [-0.515, 0.515]) m.box([x, 0, 0.34], [0.07, 0.36, 0.52], STONE);
    m.poly([[-0.48, -0.18, 0.53], [0.48, -0.18, 0.53], [0.48, 0.18, 0.53], [-0.48, 0.18, 0.53]], WATER);
    m.box([0, 0.36, 0.6], [0.26, 0.2, 1.2], DARK_STONE).box([0, 0.36, 1.23], [0.32, 0.26, 0.06], PALE_STONE).prism([[-0.13, 1.26], [0.13, 1.26], [0, 1.36]], 0.26, 0.46, STONE);
    // The spout leaves the pillar, turns down over the trough, and the jet falls from it.
    m.beam([0, 0.27, 1], [0, -0.02, 0.97], 0.05, BRASS, { sides: 5, capped: true }).beam([0, -0.02, 0.985], [0, -0.03, 0.9], 0.05, BRASS, { sides: 5, capped: true });
    return m.beam([0, -0.03, 0.9], [0, -0.03, 0.53], 0.03, JET, { capped: true });
}

/**
 * An open wooden shelter: four posts on stone feet, a wall of planks behind a bench at +x and half walls at its ends,
 * under a roof of three rows of shingles to each side of a ridge along y, on open gables.
 */
export function shelter(): Model {
    const m = new Model(), pitch = Math.tan((30 * Math.PI) / 180), eave = 2.1;
    for (const x of [-1.13, 1.13]) {
        for (const y of [-1.43, 1.43]) m.box([x, y, 0.06], [0.26, 0.26, 0.12], STONE).box([x, y, 1.08], [0.14, 0.14, 2.04], DARK_WOOD);
        m.box([x, 0, eave + 0.03], [0.14, 3, 0.12], DARK_WOOD);
    }
    // The planks of the back wall, and two boards of a half wall at each end.
    for (let k = 0; k < 8; k++) m.box([1.15, -1.19 + 0.34 * k, 1.04], [0.05, 0.33, 1.92], k % 2 ? WOOD : PALE_WOOD);
    for (const y of [-1.45, 1.45]) for (const [z, wood] of [[0.42, WOOD], [0.8, PALE_WOOD]]) m.box([0.56, y, z], [1, 0.05, 0.36], wood);
    // The bench: two planks on three legs.
    for (const [x, wood] of [[0.74, PALE_WOOD], [0.94, WOOD_LIGHT]]) m.box([x, 0, 0.45], [0.19, 2.7, 0.05], wood);
    for (const y of [-1.2, 0, 1.2]) m.box([0.86, y, 0.21], [0.36, 0.08, 0.42], DARK_WOOD);
    for (const y of [-1.43, 1.43]) {
        // An open gable: a tie beam and a post up to the ridge.
        m.box([0, y, eave + 0.03], [2.4, 0.12, 0.12], DARK_WOOD).box([0, y, eave + 0.38], [0.12, 0.12, 0.6], DARK_WOOD);
    }
    for (const side of [-1, 1]) {
        // Three rows of shingles, each over the row below it, from the eave 0.3 m past the wall to the ridge.
        for (let row = 0; row < 3; row++) {
            const [a, b] = [1.5 - 0.5 * row, 1 - 0.5 * row - (row < 2 ? 0.06 : 0)], lift = 0.035 * row, z = (x: number) => eave + (1.2 - x) * pitch + lift;
            m.prism([[side * a, z(a)], [side * b, z(b)], [side * b, z(b) + 0.09], [side * a, z(a) + 0.09]], -1.8, 1.8, row % 2 ? ROOF_LIGHT : ROOF);
        }
    }
    return m.box([0, 0, eave + 1.2 * pitch + 0.15], [0.2, 3.66, 0.1], DARK_WOOD);
}

/**
 * A bike repair station: a post with a rounded head on a slab, two arms with rubber ends that a bike hangs on, tools
 * that hang from the post on cables, and a floor pump with its handle and hose.
 */
export function bikeRepair(): Model {
    const s = new Sculpt(), m = s.model.box([0, 0, 0.02], [0.8, 0.6, 0.04], STONE);
    s.skin([[0.04, 0.085, 0.065], [1.3, 0.085, 0.065], [1.4, 0.07, 0.052], [1.44, 0.035, 0.026]].map(([z, w, d]) => ({ at: [0, 0, z] as Vec, w, up: d, down: d, pitch: 90, e: 4 })), 8, POST);
    m.box([0, -0.068, 1.12], [0.11, 0.01, 0.16], PALE_STONE);
    for (const x of [-0.05, 0.05]) {
        m.beam([x, -0.05, 1.32], [x, -0.3, 1.37], 0.035, METAL, { capped: true }).beam([x, -0.3, 1.37], [x, -0.4, 1.39], 0.05, CHARRED, { capped: true });
    }
    // Three tools on cables.
    for (const [x, z] of [[-0.04, 0.82], [0, 0.72], [0.04, 0.86]]) m.beam([x, -0.07, 1], [x, -0.075, z + 0.06], 0.01, METAL, { sides: 3 }).box([x, -0.078, z], [0.025, 0.02, 0.12], CHARRED);
    // The pump: a foot, a barrel, the rod and its handle, a gauge and the hose back to the foot of the post.
    m.box([0.28, -0.2, 0.055], [0.2, 0.08, 0.03], CHARRED).beam([0.28, -0.2, 0.06], [0.28, -0.2, 0.74], 0.1, PUMP, { sides: 6, capped: true });
    m.beam([0.28, -0.2, 0.74], [0.28, -0.2, 0.92], 0.025, METAL).box([0.28, -0.2, 0.93], [0.24, 0.035, 0.035], CHARRED).box([0.28, -0.245, 0.22], [0.08, 0.02, 0.08], PALE_STONE);
    return m.beam([0.28, -0.16, 0.12], [0.16, -0.02, 0.07], 0.02, CHARRED, { sides: 3 }).beam([0.16, -0.02, 0.07], [0.07, 0, 0.3], 0.02, CHARRED, { sides: 3 });
}
