// A wayside shrine, 1 tall with its base at the origin, z up: a whitewashed pillar, a niche that faces −y
// and a small gable roof along y.
import { rgb } from '../structures/mesh';
import { Model, roof } from './flat';

const WHITE = rgb('#e9e4d8'), NICHE = rgb('#3a3430'), ROOF = rgb('#7a4b36');

export function shrine(): Model {
    // In metres, then scaled to 1 tall.
    const m = new Model().box([0, 0, 0.9], [0.5, 0.5, 1.8], WHITE).box([0, 0, 2.1], [0.8, 0.7, 0.6], WHITE).box([0, -0.355, 2.1], [0.44, 0.02, 0.4], NICHE);
    roof(m, [0, 0, 2.4], 0.8, 0.7, 35, 0.1, ROOF, WHITE);
    const top = Math.max(...m.points.filter((_, i) => i % 3 === 2));
    return m.place(0, [0, 0, 0], 1 / top);
}
