// Grazing animals for the herds, a low-poly rework of the prototype's src/models/animals/: one
// four-legged build whose proportions, coats, horns and patches make the breed. 1 m tall at the withers'
// top, feet at the origin, z up, facing +y; the instance scales it to the breed's height.
import { srgb, Model, random, shade, type Vec } from './flat';

interface Breed {
    /** Metres to the top of the back, which the instance scales to. */
    height: number;
    /** The body's length and its half-width and half-depth, the legs' length, the neck's rise and the head's length, as shares of the height. */
    body: [number, number, number];
    leg: number;
    neck: number;
    head: number;
    /** Coats by seed: the body, and the belly, legs and head. */
    coats: [string, string][];
    /** Patches of the second colour over the body, a share of its faces. */
    patches?: number;
    horns?: 'cow' | 'long' | 'curl' | 'goat';
    hooves: string;
}

/** The breeds that the packs of data/trailview/packs name as herd models. */
export const BREEDS: Record<string, Breed> = {
    'brown-cattle': { height: 1.4, body: [1.45, 0.3, 0.32], leg: 0.52, neck: 0.12, head: 0.36, coats: [['#7e6a58', '#a8957e'], ['#8e4a24', '#b87444'], ['#ae8250', '#d4ae78']], horns: 'cow', hooves: '#2c2420' },
    'dairy-cattle': { height: 1.45, body: [1.5, 0.29, 0.32], leg: 0.55, neck: 0.1, head: 0.36, coats: [['#e9e6de', '#1e1c1b'], ['#e9e6de', '#1e1c1b']], patches: 0.45, hooves: '#2c2420' },
    'simmental-cattle': { height: 1.45, body: [1.45, 0.3, 0.32], leg: 0.52, neck: 0.1, head: 0.36, coats: [['#ece6da', '#a85428'], ['#ece6da', '#c88e4c']], patches: 0.5, horns: 'cow', hooves: '#2c2420' },
    'highland-cow': { height: 1.3, body: [1.4, 0.34, 0.36], leg: 0.46, neck: 0.1, head: 0.38, coats: [['#b3561f', '#e09048'], ['#4a2a1a', '#7a4a2c'], ['#d0ae74', '#efd6a2']], horns: 'long', hooves: '#2c2420' },
    sheep: { height: 0.95, body: [1.15, 0.36, 0.38], leg: 0.5, neck: 0.14, head: 0.3, coats: [['#efe7d6', '#2d2828'], ['#e6dcc6', '#2d2828']], horns: 'curl', hooves: '#2e211b' },
    goat: { height: 1.05, body: [1.1, 0.24, 0.27], leg: 0.6, neck: 0.22, head: 0.3, coats: [['#2e241e', '#ece4d4'], ['#2a211c', '#5a4434'], ['#87482a', '#e6dccb']], horns: 'goat', hooves: '#2c2420' },
    horse: { height: 1.6, body: [1.35, 0.22, 0.27], leg: 0.62, neck: 0.42, head: 0.42, coats: [['#6a3418', '#1a1512'], ['#8a4018', '#6e3014'], ['#b8b6b2', '#6e6c6a'], ['#221f1e', '#141212']], hooves: '#2a2624' },
};

/** An animal of a breed in one of its coats. */
export function animal(name: string, seed = 0): Model {
    const b = BREEDS[name], [length, half, deep] = b.body, rand = random(seed + 3);
    const [coat, other] = b.coats[seed % b.coats.length].map(srgb), m = new Model();
    const back = 1, belly = b.leg, mid = (back + belly) / 2, y0 = -length / 2, y1 = length / 2;
    // The body: an octagon swept from rump to chest; patches paint some of its faces in the second colour.
    const ring = (y: number, k: number): Vec[] => Array.from({ length: 8 }, (_, i) => {
        const a = (i / 8) * 2 * Math.PI;
        return [Math.cos(a) * half * k, y, mid + Math.sin(a) * (back - belly) / 2 * k];
    });
    const rings = [ring(y0, 0.75), ring(y0 + 0.15 * length, 1), ring(y1 - 0.2 * length, 1), ring(y1, 0.8)];
    rings.slice(1).forEach((r, k) => r.forEach((p, i) => {
        const q = rings[k], j = (i + 1) % 8, low = Math.sin(((i + 0.5) / 8) * 2 * Math.PI) < -0.5;
        m.poly([q[i], q[j], r[j], p], b.patches && rand() < b.patches ? other : low && !b.patches ? shade(coat, 0.85) : coat);
    }));
    m.poly(rings[0], coat).poly(rings[3], coat);
    // Neck and head: the neck rises from the chest, the head hangs forward from it.
    const neck: Vec = [0, y1 + 0.05, back + b.neck], snout: Vec = [0, y1 + 0.05 + b.head, back + b.neck - 0.25 * b.head];
    const face = b.patches ? other : name === 'sheep' ? other : coat;
    m.beam([0, y1 - 0.05, mid + 0.05], neck, deep * 1.1, coat, { sides: 6, w1: deep * 0.8 });
    m.beam(neck, snout, deep * 0.8, face, { sides: 6, w1: deep * 0.5, capped: true });
    for (const s of [-1, 1]) m.box([s * deep * 0.5, neck[1] + 0.03, neck[2] + 0.06], [0.14 * b.head + 0.05, 0.04, 0.06], face);
    // Legs, hooves and tail.
    for (const [x, y] of [[-1, y0 + 0.12], [1, y0 + 0.12], [-1, y1 - 0.12], [1, y1 - 0.12]]) {
        m.beam([x * half * 0.55, y, mid], [x * half * 0.55, y, 0.06], 0.09 + 0.05 * half, name === 'sheep' || b.patches ? other : coat, { sides: 4, w1: 0.07 });
        m.box([x * half * 0.55, y, 0.03], [0.1, 0.12, 0.06], srgb(b.hooves));
    }
    m.beam([0, y0, back - 0.05], [0, y0 - 0.06, belly - 0.05], 0.05, coat, { sides: 3 });
    // Horns.
    const horn = srgb('#e2d6b8'), top = neck[2] + 0.1;
    if (b.horns) for (const s of [-1, 1]) {
        const at: Vec = [s * deep * 0.35, neck[1] + 0.02, top];
        const tip: Vec = b.horns === 'long' ? [s * (deep + 0.45), at[1] + 0.05, top + 0.12] : b.horns === 'goat' ? [s * 0.12, at[1] - 0.22, top + 0.2]
            : b.horns === 'curl' ? [s * (deep * 0.6), at[1] - 0.05, top - 0.12] : [s * (deep * 0.5 + 0.12), at[1] + 0.04, top + 0.12];
        m.beam(at, tip, 0.06, b.horns === 'goat' ? srgb('#6b5c4a') : horn, { sides: 3, w1: 0.02 });
    }
    return m;
}
