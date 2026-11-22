import type { Plant } from '../plant-library';
import { across, GOLDEN, hex, kite, ramp, random, sheet, spike, spindle, UP, v3, type Colour, type Geometry, type Ramp, type Sheet, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** The rosette's keeled leaves: grey-green from autumn to spring, dead straw by summer. */
const LEAVES: Record<Season, Ramp> = {
    winter: ['#4a6444', '#7c9a6c', '#a6bc92'],
    spring: ['#4a6444', '#7c9a6c', '#a6b088'],
    summer: ['#6a5034', '#9c7e52', '#c6aa7a'],
    autumn: ['#4e7044', '#84a86c', '#aac890'],
};

/** Leaf length in metres of a one-metre plant, and how far from upright it leans and droops. */
const ROSETTE: Record<Season, [length: number, tilt: number, arch: number]> = {
    winter: [0.5, 0.75, 0.35],
    spring: [0.55, 0.85, 0.5],
    summer: [0.45, 1.25, 0.5],
    autumn: [0.28, 0.6, 0.25],
};

/**
 * The stalk, the colour of what sits along its racemes and the buds at their tips. Winter has the
 * new stalk in bud, spring white stars, summer the dry stalk with brown capsules, autumn last
 * year's stalk weathered grey.
 */
const STALK: Record<Season, [stalk: string, along: string, buds: string]> = {
    winter: ['#6a8650', '#7e8a5a', '#9a6e60'],
    spring: ['#6a8650', '#fbf8f2', '#b07c6c'],
    summer: ['#8a6c48', '#6a5034', '#8a6e48'],
    autumn: ['#8e877a', '#7c7468', '#a49884'],
};

/** A star flower at `at`, facing `n`: five pointed tepals that rise `cup` radii to their tips, `throat` where they meet. 5 triangles. */
function star(cards: Sheet, at: V, n: V, r: number, cup: number, throat: Colour, petal: Colour, turn: number): void {
    const [u, w] = across(n);
    // Lit as if turned halfway to the sky, so a raceme's stars stay bright from every side.
    const light = () => n.clone().lerp(UP, 0.5).normalize();
    for (let k = 0; k < 5; k++) {
        const a = turn + (k / 5) * 2 * Math.PI;
        const dir = u.clone().multiplyScalar(Math.cos(a)).addScaledVector(w, Math.sin(a)), half = v3().crossVectors(n, dir).multiplyScalar(0.36 * r);
        cards.add([at.clone().add(half), at.clone().sub(half), at.clone().addScaledVector(dir, r).addScaledVector(n, cup * r)], [throat, throat, petal], light);
    }
}

/**
 * Branched asphodel: a fountain of long keeled leaves, and out of it one stalk that branches near
 * the top into a candelabra of racemes. In spring the racemes carry white stars under a tip in
 * bud, in winter a slim cluster of buds, in summer and autumn round seed capsules; the stalk keeps
 * its height in every season. 175 triangles.
 */
function asphodel(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(LEAVES[season]);
    const [stalk, along, buds] = STALK[season].map(hex);
    const [length, tilt, arch] = ROSETTE[season];
    const cards = sheet();
    // Spring leaves have dried at the tips by flowering time.
    const dryTip = season === 'spring' ? hex('#a88a5a') : tip;
    for (let k = 0; k < 20; k++) {
        const a = k * GOLDEN + rand() * 0.4, l = length * (0.7 + 0.4 * rand()), lean = tilt * (0.45 + 0.75 * rand()), drop = arch * (0.8 + 0.4 * rand());
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const on = (s: number) => out.clone().multiplyScalar(Math.sin(lean) * s * l).setZ((Math.cos(lean) * s - drop * s * s) * l);
        kite(cards, v3(), on(0.5), on(1), v3(-out.y, out.x, 0).multiplyScalar(0.045), [deep, mid, dryTip], (p) => out.clone().multiplyScalar(p.length() / l).add(UP).normalize());
    }
    const lean = rand() * 2 * Math.PI;
    const top = v3(0.05 * Math.cos(lean), 0.05 * Math.sin(lean), 1);
    const tips = season === 'spring' ? buds : stalk;
    spike(cards, v3(), top, 0.024, [stalk.clone().lerp(deep, 0.3), tips]);
    const racemes: [V, V, number][] = [[v3().lerp(top, 0.58), top, 8]];
    for (let b = 0; b < 4; b++) {
        const at = v3().lerp(top, 0.46 + 0.06 * b + 0.04 * rand());
        const a = lean + Math.PI / 2 + b * 1.7 + rand() * 0.5;
        const reach = 0.18 + 0.09 * rand();
        const end = at.clone().add(v3(reach * Math.cos(a), reach * Math.sin(a), 0.3 + 0.12 * rand()));
        spike(cards, at, end, 0.014, [stalk, tips]);
        racemes.push([at.clone().lerp(end, 0.3), end, season === 'spring' ? 4 : 3]);
    }
    for (const [from, to, count] of racemes) {
        const dir = to.clone().sub(from).normalize();
        if (season === 'winter') {
            spindle(cards, from.clone().lerp(to, 0.4), to, 0.026, 5, [along, buds, buds]);
            continue;
        }
        // Flowers or capsules from the raceme's foot to five sixths up, turning round it.
        for (let i = 0; i < count; i++) {
            const turn = i * GOLDEN * 2 + rand();
            const out = v3(Math.cos(turn), Math.sin(turn), 0);
            const at = from.clone().lerp(to, (0.84 * (i + 0.5)) / count);
            const n = out.clone().multiplyScalar(0.8).addScaledVector(dir, 0.6).normalize();
            if (season === 'spring') star(cards, at.addScaledVector(out, 0.012), n, 0.06 * (0.85 + 0.3 * rand()), 0.35, along.clone().lerp(buds, 0.25), along, rand() * 2);
            else spindle(cards, at, at.clone().addScaledVector(n, 0.062), 0.024, 3, [stalk, along, buds]);
        }
    }
    return cards.build();
}

export default {
    name: 'Branched asphodel',
    latin: 'Asphodelus ramosus',
    build: (season, seed) => asphodel(season, random(seed)),
    ground: { on: ['grass', 'bare'], height: [0.5, 1.0], share: 0.05, own: true, clumped: true },
} satisfies Plant;
