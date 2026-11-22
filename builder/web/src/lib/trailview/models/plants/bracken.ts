import type { Plant } from '../plant-library';
import { hex, ramp, random, sheet, smooth, spike, UP, v3, type Colour, type Geometry, type Ramp, type Sheet, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Bracken: fresh green, rust-orange as it dies in autumn, faded russet through winter. */
const BRACKEN: Record<Season, Ramp> = {
    winter: ['#55301e', '#8a4e2e', '#b27a4c'],
    spring: ['#3f6a24', '#72a43a', '#b0d266'],
    summer: ['#2f5a20', '#5a9030', '#9ac24c'],
    autumn: ['#70300f', '#b8581c', '#e69a44'],
};

/**
 * How far bracken has died back, for every other frond and the ones between: up to 0.35 the blade
 * withers and hangs, beyond that the stalk bends over. The standing fronds keep a clump's height,
 * so it keeps its size through the seasons. Spring's fallen fronds are last year's.
 */
const BRACKEN_SLUMP: Record<Season, [number, number]> = { winter: [0.35, 1], spring: [0, 1], summer: [0, 0], autumn: [0.08, 0.22] };

/**
 * A feather along `path`, level across `side`: `pairs` broad teeth on each flank, swept forward,
 * `reach(t)` long and drooping `droop` of that. Those of one flank stand half a step past those of
 * the other, so the feather is whole along its length, with a ridge. Dark at its foot, light at its
 * tip and its edge. 2 × `pairs` triangles.
 */
function feather(cards: Sheet, path: (s: number) => V, side: V, pairs: number, reach: (t: number) => number, droop: number, [deep, mid, tip]: Colour[]): void {
    const colour = (s: number) => deep.clone().lerp(mid, smooth(0, 0.25, s)).lerp(tip, 0.7 * smooth(0.25, 1, s));
    for (const flank of [-1, 1]) for (let i = 0; i < pairs; i++) {
        const [s0, s1] = [i, i + 1].map((k) => (k + (flank > 0 ? 0.5 : 0)) / (pairs + 0.5)), t = (s0 + s1) / 2;
        const [from, to] = [path(s0), path(s1)];
        const up = v3().crossVectors(side, to.clone().sub(from)).normalize();
        if (up.z < 0) up.negate();
        const end = from.clone().lerp(to, 0.75).addScaledVector(side, flank * reach(t)).addScaledVector(up, -droop * reach(t));
        end.z = Math.max(0.01, end.z);
        const light = (p: V) => (p === end ? up.clone().addScaledVector(side, 0.6 * flank) : up.clone()).normalize().lerp(UP, 0.3).normalize();
        cards.add([from, to, end], [colour(s0), colour(s1), colour(t).lerp(tip, 0.6)], light);
    }
}

/**
 * A bracken frond: a stalk that leans out of the ground, and on it a broad triangular blade of
 * three feathers that rises and arches over: the rachis, and one branch to each side at its foot.
 * `slump` kills it, as BRACKEN_SLUMP says, until a fallen blade lies on the ground near the root;
 * a low `open` holds a young blade steep and narrow, its branches not yet spread. 27 triangles.
 */
function frond(cards: Sheet, root: V, a: number, size: number, colours: Colour[], slump: number, open: number, rand: () => number): void {
    const out = v3(Math.cos(a), Math.sin(a), 0), side = v3(-out.y, out.x, 0);
    const [hang, fall, young] = [smooth(0, 0.35, slump), smooth(0.35, 1, slump), 1 - open];
    const height = size * (0.42 + 0.12 * rand()) * (1 - 0.8 * fall);
    const top = root.clone().addScaledVector(out, height * (0.2 + 0.15 * rand() + 2 * fall)).setZ(height);
    const length = size * (0.9 + 0.16 * rand()) * (0.6 + 0.4 * open) * (1 - 0.25 * hang);
    const reach = length * open * (1 - 0.3 * hang), fold = 0.15 + 0.35 * hang;
    const pitch = 0.45 + 0.15 * rand() + 1.3 * young - 0.45 * hang - 0.6 * fall, droop = 0.32 - 0.4 * young + 0.3 * hang;
    // A parabola from the stalk's top: a live tip ends near the top's height, a withered one hangs to the ground.
    const arch = (from: V, dir: V, l: number, sag: number) => (u: number) =>
        from.clone().addScaledVector(dir, l * Math.cos(pitch) * u).setZ(Math.max(0.02, from.z + l * (Math.sin(pitch) * u - sag * u * u)));
    const rachis = arch(top, out, length, droop), blade = rachis(0.12);
    spike(cards, root, blade, 0.022 * size, [colours[0], colours[0].clone().lerp(colours[1], 0.5)]);
    feather(cards, (s) => rachis(0.12 + 0.88 * s), side, 6, (t) => 0.3 * reach * (1 - t) ** 0.6, fold, colours);
    const spread = 1.05 * open ** 2 * (1 - 0.5 * hang);
    for (const flank of [-1, 1]) {
        const dir = out.clone().multiplyScalar(Math.cos(spread)).addScaledVector(side, flank * Math.sin(spread));
        const branch = arch(blade, dir, 0.6 * length, droop + 0.1);
        feather(cards, branch, v3(-dir.y, dir.x, 0), 3, (t) => 0.21 * reach * (1 - t) ** 0.6, fold, colours);
    }
}

/**
 * Six bracken fronds arching out of a small patch, their blades in tiers. In spring young fronds
 * stand among last year's, which lie on the ground. A clump is the same in every season, only
 * older. 162 triangles.
 */
function bracken(season: Season, rand: () => number): Geometry {
    const cards = sheet();
    for (let k = 0; k < 6; k++) {
        const fallen = k % 2 === 1;
        const old = season === 'spring' && fallen;
        const b = (k / 6) * 2 * Math.PI + rand() * 0.8;
        const d = 0.05 + 0.2 * rand();
        const colours = ramp(BRACKEN[old ? 'winter' : season]);
        // Autumn fronds die unevenly: some still orange, some already brown.
        const brown = 0.6 * rand();
        const shade = 0.85 + 0.3 * rand();
        colours.forEach((c, i) => c.lerp(hex(BRACKEN.winter[i]), season === 'autumn' ? brown : 0).multiplyScalar(shade));
        const slump = Math.min(1, BRACKEN_SLUMP[season][fallen ? 1 : 0] * (0.85 + 0.3 * rand()));
        const open = season === 'spring' && !old ? 0.55 : 1;
        // Lower fronds around a tall one hide its stalk under their blades, in tiers.
        const size = 1 - (0.55 * k * (0.8 + 0.4 * rand())) / 6;
        frond(cards, v3(d * Math.cos(b), d * Math.sin(b), 0), b + (rand() - 0.5) * 0.6, size, colours, slump, open, rand);
    }
    return cards.build();
}

export default {
    name: 'Bracken',
    latin: 'Pteridium aquilinum',
    build: (season, seed) => bracken(season, random(seed)),
    ground: { on: ['grass', 'shrub'], height: [0.6, 1.1], share: 0.12, own: true, clumped: true },
} satisfies Plant;
