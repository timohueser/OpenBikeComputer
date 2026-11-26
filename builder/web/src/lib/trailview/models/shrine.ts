// A wayside shrine, 1 tall with its base at the origin, z up: a whitewashed pillar on a stone foot, a head with a niche
// that faces −y, and a small gable roof along y.
import { rgb } from '../structures/mesh';
import { Model, roof } from './flat';

const WHITE = rgb('#e9e4d8'), SHADE = rgb('#d6d0c2'), STONE = rgb('#9a958c'), NICHE = rgb('#3a3430'), ROOF = rgb('#7a4b36'), GOLD = rgb('#c9a23c'), BLOOM = rgb('#c8402a'), POT = rgb('#8a5a3c');

export function shrine(): Model {
    // In metres, then scaled to 1 tall.
    const m = new Model().box([0, 0, 0.12], [0.62, 0.62, 0.24], STONE).box([0, 0, 0.99], [0.5, 0.5, 1.62], WHITE).box([0, 0, 1.78], [0.8, 0.7, 0.06], STONE).box([0, 0, 2.105], [0.76, 0.66, 0.59], WHITE);
    // The niche: a dark opening under a pointed arch, with a small cross of gold in it, and a pot of flowers on the ledge before it.
    m.box([0, -0.335, 2.03], [0.4, 0.02, 0.32], NICHE).prism([[-0.2, 2.19], [0.2, 2.19], [0, 2.33]], -0.345, -0.325, NICHE);
    m.box([0, -0.35, 2.08], [0.035, 0.012, 0.2], GOLD).box([0, -0.35, 2.12], [0.12, 0.012, 0.035], GOLD);
    m.box([0.27, -0.31, 1.85], [0.07, 0.07, 0.08], POT).box([0.27, -0.31, 1.92], [0.1, 0.1, 0.07], BLOOM);
    m.box([0, 0, 2.4], [0.8, 0.7, 0.02], SHADE);
    roof(m, [0, 0, 2.4], 0.8, 0.7, 35, 0.1, ROOF, WHITE);
    const top = Math.max(...m.points.filter((_, i) => i % 3 === 2));
    return m.place(0, [0, 0, 0], 1 / top);
}
