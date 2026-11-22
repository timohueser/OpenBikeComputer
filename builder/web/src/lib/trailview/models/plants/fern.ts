import type { Plant } from '../plant-library';
import { merge, paint, ramp, random, ribbon, sheet, smooth, UP, v3, type Colour, type Geometry, type Ramp, type Sheet, type V, type Vec } from '../plant-shape';
import type { Season } from '../../rules/regions';

const FERN: Record<Season, Ramp> = {
    winter: ['#5a4030', '#7a5638', '#94704a'],
    spring: ['#4f8a3a', '#7ab84e', '#b0dc72'],
    summer: ['#2f5e2a', '#4a8236', '#7aab4c'],
    autumn: ['#7a4a22', '#a8682c', '#cf9448'],
};

/**
 * A frond from the crown toward angle `a`, `length` along the ground: it leaves the crown steeply,
 * arches out and ends level. `pairs` pinnae sit on each side of the rachis, the longest below the
 * middle, each a broad tooth that droops from the rachis. Those of one side stand half a step past
 * those of the other, so the frond is whole along its length: a zigzag feather with a ridge. It is
 * dark at the crown and light at its tip and its edge.
 */
function frond(cards: Sheet, a: number, length: number, rise: number, pairs: number, [deep, mid, tip]: Colour[]): void {
    const out = v3(Math.cos(a), Math.sin(a), 0), across = v3(-out.y, out.x, 0);
    const at = (s: number) => out.clone().multiplyScalar((0.45 * s + 0.55 * s * s) * length).setZ((1.7 * s - 0.7 * s * s) * rise);
    const colour = (s: number) => deep.clone().lerp(mid, smooth(0, 0.2, s)).lerp(tip, 0.8 * smooth(0.2, 0.9, s));
    for (const side of [-1, 1]) for (let i = 0; i < pairs; i++) {
        const [s0, s1] = [i, i + 1].map(k => (k + (side > 0 ? 0.5 : 0)) / (pairs + 0.5)), t = (s0 + s1) / 2;
        const [from, to] = [at(s0), at(s1)];
        const up = v3().crossVectors(across, to.clone().sub(from)).normalize();
        const reach = 0.36 * length * smooth(-0.1, 0.35, t) * (1 - t) ** 0.5;
        const end = at(t + 0.2 * (s1 - s0)).addScaledVector(across, side * reach).addScaledVector(up, -0.3 * reach);
        const light = (p: V) => (p === end ? up.clone().addScaledVector(across, 0.6 * side) : up.clone()).normalize().lerp(UP, 0.3).normalize();
        cards.add([from, to, end], [colour(s0), colour(s1), colour(t).lerp(tip, 0.6)], light);
    }
}

/**
 * Male fern: a shuttlecock of six arching fronds round three young ones that stand in the middle.
 * In spring the young ones are crosiers, still rolled up at the tip.
 */
function fern(season: Season, rand: () => number): Geometry {
    const colours = ramp(FERN[season]);
    const cards = sheet(), parts: Geometry[] = [];
    for (let k = 0; k < 6; k++) frond(cards, (k / 6) * 2 * Math.PI + rand() * 0.5, 0.78 + 0.2 * rand(), 0.85 + 0.15 * rand(), 6, colours);
    for (let k = 0; k < 3; k++) {
        const a = ((k + 0.5) / 3) * 2 * Math.PI + rand() * 0.8, h = 0.6 + 0.15 * rand();
        if (season !== 'spring') {
            frond(cards, a, 0.3 + 0.1 * rand(), h, 3, colours);
            continue;
        }
        // The stalk leans out, and its tip rolls back over toward the crown.
        const crook = ([d, z]: [number, number]): Vec => [h * d * Math.cos(a), h * d * Math.sin(a), h * z];
        const path = ([[0, 0], [0.14, 0.5], [0.24, 0.92], [0.1, 1], [0.08, 0.8]] as [number, number][]).map(crook);
        parts.push(paint(ribbon(path, [-Math.sin(a), Math.cos(a), 0], (t) => 0.06 + 0.07 * t, 3, 0.3), (p) => colours[1].clone().lerp(colours[2], smooth(0, h, p.z))));
    }
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Fern',
    latin: 'Dryopteris filix-mas',
    build: (season, seed) => fern(season, random(seed)),
    ground: { on: ['tree'], height: [0.5, 0.9], share: 0.3 },
} satisfies Plant;
