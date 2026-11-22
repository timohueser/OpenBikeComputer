import { rgb } from '../../structures/mesh';
import { shade } from '../flat';
import type { Plant } from '../plant-library';
import { merge, place, random, type Geometry, type Vec } from '../plant-shape';
import { mesh, Sculpt, through } from '../sculpt';
import type { Season } from '../../rules/regions';
import cardon from './cardon';

/** Cardón stems, their shoulders and their tips: glaucous grey-green after the winter rains, yellower in the drought. */
const CARDON: Record<Season, [number, number, number]> = {
    winter: [rgb('#56765a'), rgb('#7c9a70'), rgb('#a0b488')],
    spring: [rgb('#5a7a5c'), rgb('#80a074'), rgb('#a6b88a')],
    summer: [rgb('#687652'), rgb('#8c9666'), rgb('#aaac7c')],
    autumn: [rgb('#5a7858'), rgb('#7e9a70'), rgb('#a2b286')],
};

/** Red cyathia round the stem tips in spring, red-brown capsules in summer. */
const CARDON_FRUIT: Partial<Record<Season, number>> = { spring: rgb('#c2443a'), summer: rgb('#96503a') };

/** The stems of the middle bundle and of the ring round it, and the fruits of a stem in flower. */
const INNER = 4, OUTER = 5, FRUITS = 3;

/**
 * A five-sided stem of flat facets through `points`, which bend toward +y, then turned `a` about
 * the plant's middle: `r` thick, with a bevelled shoulder under a small flat tip. With `fruit`, a
 * crown of three small knobs sits on the bevel.
 */
function stem(points: Vec[], r: number, a: number, [body, shoulder, tip]: number[], fruit?: number): Geometry {
    const s = new Sculpt(), top = points[points.length - 1], crown: Vec = [top[0], top[1], top[2] + 0.7 * r];
    const rows = [...points.map((_, i) => (i ? r : 0.8 * r)), 0.5 * r].map(w => [w, w, w]);
    s.skin(through([...points, crown], rows), 5, band => (band < 0 ? body : band >= points.length ? tip : band === points.length - 1 ? shoulder : body));
    if (fruit !== undefined) for (let k = 0; k < FRUITS; k++) {
        const t = (k / FRUITS) * 2 * Math.PI + 0.5, [x, y, size] = [Math.cos(t), Math.sin(t), 0.3 * r];
        // The knob's frame on the bevel: out and up of the stem, along the bevel, and round the stem.
        const at = (out: number, slope: number, round: number): Vec => [top[0] + (0.6 * r + 0.7 * (out - slope)) * x - round * y, top[1] + (0.6 * r + 0.7 * (out - slope)) * y + round * x, top[2] + 0.5 * r + 0.7 * (out + slope)];
        const foot = [0, 1, 2].map(j => at(0, size * Math.cos((j * 2 * Math.PI) / 3), size * Math.sin((j * 2 * Math.PI) / 3)));
        for (let j = 0; j < 3; j++) s.face([foot[j], foot[(j + 1) % 3], at(1.3 * size, 0, 0)], fruit, at(0, 0, 0));
    }
    return place(mesh(s.model) as Geometry, { rot: [0, 0, a] });
}

/**
 * The cardón in the animals' style: a candelabra of nine thick five-sided stems in flat facets.
 * Four stand in a tight bundle in the middle; five round them leave the base sideways and bend
 * upright at an elbow. In spring and summer the middle stem and one outer stem carry fruits.
 */
function faceted(season: Season, rand: () => number): Geometry {
    const fruit = CARDON_FRUIT[season], parts: Geometry[] = [];
    const tones = () => CARDON[season].map(c => shade(c, 0.9 + 0.2 * rand()));
    for (let k = 0; k < INNER; k++) {
        const d = k ? 0.2 + 0.03 * rand() : 0.02, r = 0.115 * (0.92 + 0.16 * rand());
        parts.push(stem([[0, 0.7 * d, -0.03], [0, d + 0.03, 0.88 - 0.5 * d - 0.12 * rand()]], r, (k / (INNER - 1)) * 2 * Math.PI + rand() * 0.3, tones(), k ? undefined : fruit));
    }
    for (let k = 0; k < OUTER; k++) {
        const d = 0.42 + 0.08 * rand(), r = 0.115 * (0.92 + 0.16 * rand());
        const points: Vec[] = [[0, 0.14, 0], [0, d, 0.2 + 0.05 * rand()], [0, d + 0.04, 0.42 + 0.22 * rand()]];
        parts.push(stem(points, r, ((k + 0.5) / OUTER) * 2 * Math.PI + (rand() - 0.5) * 0.3, tones(), k === 2 ? fruit : undefined));
    }
    return merge(parts);
}

export default {
    name: 'Cardón (faceted)',
    latin: cardon.latin,
    build: (season, seed) => faceted(season, random(seed)),
    ground: cardon.ground,
} satisfies Plant;
