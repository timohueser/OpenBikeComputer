import type { Plant } from '../plant-library';
import { byHeight, circle, merge, paint, place, ramp, random, ribbon, sheet, soft, v3, type Geometry, type Ramp, type V, type Vec } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** The palmate leaves: mid green, dead brown in winter. */
const LEAVES: Record<Season, Ramp> = {
    winter: ['#4a3a2a', '#6a5440', '#8a7058'],
    spring: ['#355a2c', '#527a40', '#7ca062'],
    summer: ['#33562a', '#4e763e', '#76985c'],
    autumn: ['#4a5a2c', '#6e7a3e', '#94985a'],
};

/** The spikes: blue-purple in flower in early summer, green pods after, then dry brown. */
const SPIKES: Record<Season, Ramp> = {
    winter: ['#4a3828', '#6a5038', '#8a6c4c'],
    spring: ['#3e3088', '#5a4cb0', '#9a90d8'],
    summer: ['#4a6a30', '#6a8a44', '#5a4cb0'],
    autumn: ['#5a4630', '#7a6040', '#9a7c54'],
};

/** A palmate leaf: six leaflets from one point, twelve triangles. */
function palm(r: number): Geometry {
    const geo = circle(r, 12);
    for (let i = 2; i < geo.position.length / 3; i += 2) geo.position.splice(3 * i, 3, geo.position[3 * i] * 0.3, geo.position[3 * i + 1] * 0.3, 0);
    return soft(geo);
}

/**
 * Nootka lupine: a bushy clump of palmate leaves on stalks, with six dense flower spikes above it.
 * A spike is four crossed triangles round its stalk, wide at the foot and pointed at the top, so it
 * reads as a cone from every side. 140 triangles.
 */
function lupine(season: Season, rand: () => number): Geometry {
    const leaf = byHeight(ramp(LEAVES[season]), 0.6);
    const [deep, mid, tip] = ramp(SPIKES[season]);
    const parts: Geometry[] = [];
    for (let k = 0; k < 8; k++) {
        const a = (k / 8) * 2 * Math.PI + rand();
        const d = 0.1 + 0.25 * rand();
        const h = 0.25 + 0.3 * rand();
        const at: Vec = [d * Math.cos(a), d * Math.sin(a), h];
        parts.push(paint(ribbon([[0, 0, 0], at], [-Math.sin(a), Math.cos(a), 0], () => 0.015, 1), leaf));
        parts.push(paint(place(palm(0.16), { at, rot: [(rand() - 0.5) * 0.6, (rand() - 0.5) * 0.6, rand() * 6] }), leaf));
    }
    const cards = sheet();
    for (let k = 0; k < 6; k++) {
        const a = (k / 6) * 2 * Math.PI + rand();
        const d = 0.05 + 0.2 * rand();
        const foot = v3(d * Math.cos(a), d * Math.sin(a), 0.45 + 0.1 * rand());
        const top = foot.clone().add(v3(0.03 * Math.cos(a), 0.03 * Math.sin(a), 0.4 + 0.15 * rand()));
        parts.push(paint(ribbon([[0, 0, 0], foot.toArray()], [-Math.sin(a), Math.cos(a), 0], () => 0.02, 1), leaf));
        const light = (p: V) => v3(p.x - foot.x, p.y - foot.y, 0.5).normalize();
        for (let i = 0; i < 4; i++) {
            const t = a + (i * Math.PI) / 4;
            const side = v3(0.06 * Math.cos(t), 0.06 * Math.sin(t), 0);
            cards.add([foot.clone().add(side), foot.clone().sub(side), top], [deep, mid, tip], light);
        }
    }
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Nootka lupine',
    latin: 'Lupinus nootkatensis',
    build: (season, seed) => lupine(season, random(seed)),
    ground: { on: ['bare', 'grass', 'shrub'], height: [0.5, 1], share: 0.3, own: true, clumped: true },
} satisfies Plant;
