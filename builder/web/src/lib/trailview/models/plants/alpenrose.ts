import type { Plant } from '../plant-library';
import { across, cushion, domeAt, domed, hash, hex, leaf, merge, onDome, ramp, random, sheet, tufts, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Alpenrose leaves: dark glossy evergreen, bronzed by the winter. */
const ALPENROSE: Record<Season, Ramp> = {
    winter: ['#22301e', '#3c4e2e', '#6a7646'],
    spring: ['#1e3420', '#36582e', '#74984e'],
    summer: ['#1e3220', '#34562e', '#6c924c'],
    autumn: ['#20321f', '#36522d', '#6a8a48'],
};

/** Share of alpenrose leaves turned to show their rust-brown undersides. */
const ALPENROSE_RUST: Record<Season, number> = { winter: 0.3, spring: 0.15, summer: 0.12, autumn: 0.2 };

const ALPENROSE_FLOWER: Ramp = ['#a81e40', '#d0305a', '#f27094'];

/**
 * Alpenrose: a cushion of four dark domes about twice as wide as tall, with rosettes of four leathery leaves on
 * the twig tips all over it; a few leaves show their rusty undersides or a glossy light. In summer a pink-red
 * flower truss sits in the middle of nine rosettes, lower than their leaves, so the bush keeps its size when scaled
 * to 1 tall.
 */
function alpenrose(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(ALPENROSE[season]);
    const [dark, bloom, lit] = ramp(ALPENROSE_FLOWER);
    const [rust, gloss] = [hex('#7a4a30'), hex('#b4ccaa')];
    const cards = sheet();
    const mounds = tufts([0, 0, 0.52, 0.78], 3, 0.7, [0.5, 0.4, 0.5], [0.1, 0.08, 0.14], rand);
    const parts = mounds.map((mound, k) => cushion(mound, k ? 6 : 8, [deep.clone().multiplyScalar(0.75), deep.clone().lerp(mid, 0.4), mid], 0.3, seed + k));
    const sites: [V, V][] = [];
    mounds.forEach((mound, k) => {
        const [x, y, r, h] = mound, light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
        const rosettes = k ? 3 : 5, turn = rand() * 2 * Math.PI, sun = rand() < 0.3 ? 0.5 : 0.2 * rand();
        for (let j = 0; j < rosettes; j++) {
            // A side dome has one rosette on its top and two on its open flank, where the middle dome does not hide them.
            const [at, out] = k ? domeAt(mound, Math.atan2(y, x) + (j - 1) * (0.9 + 0.3 * rand()), j === 1 ? 0.3 : 1.05) : onDome(mound, j, rosettes, 1.25, turn);
            const [u, w] = across(out), spin = rand() * Math.PI;
            for (let i = 0; i < 4; i++) {
                const t = spin + (i + 0.3 * rand()) * (Math.PI / 2);
                // The leaves cup up round the twig tip.
                const dir = u.clone().multiplyScalar(Math.cos(t)).addScaledVector(w, Math.sin(t)).multiplyScalar(0.82).addScaledVector(out, 0.57);
                const body = mid.clone().lerp(tip, sun + 0.25 * rand());
                const end = body.clone().lerp(tip, 0.4), roll = rand();
                if (roll < ALPENROSE_RUST[season]) end.lerp(rust, 0.7);
                else if (roll > 0.85) end.lerp(gloss, 0.5);
                leaf(cards, at, dir, 0.32 + 0.06 * rand(), 0.16, 0.55, [deep, body, end], light);
            }
            sites.push([at, out]);
        }
    });
    // A truss: a low six-sided cone in a rosette, lit round its own middle. The rosettes are picked by hash rather
    // than rand, so a bush is the same in every season.
    if (season === 'summer') for (const [at, out] of sites.sort((p, q) => hash(p[0], 7) - hash(q[0], 7)).slice(0, 9)) {
        const [u, w] = across(out), own = domed(at, v3(1, 1, 1), 0.2);
        const crown = at.clone().addScaledVector(out, 0.14);
        const rim = Array.from({ length: 7 }, (_, i) => at.clone().addScaledVector(out, 0.05).addScaledVector(u, 0.12 * Math.cos((i * Math.PI) / 3)).addScaledVector(w, 0.12 * Math.sin((i * Math.PI) / 3)));
        for (let i = 0; i < 6; i++) cards.add([crown, rim[i], rim[i + 1]], [lit, i % 2 ? dark : bloom, i % 2 ? bloom : dark], own);
    }
    return merge([...parts, cards.build()]);
}

export default {
    name: 'Alpenrose',
    latin: 'Rhododendron ferrugineum',
    build: (season, seed) => alpenrose(season, seed, random(seed)),
    ground: { on: ['shrub', 'grass'], height: [0.4, 0.9], share: 0.1, own: true, clumped: true },
} satisfies Plant;
