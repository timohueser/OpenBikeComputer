// The crowns of odd broadleaf forms: a shrub of rods, a palm and a dragon tree.

import { domed, GOLDEN, hex, lathe, lump, paint, place, sheet, smooth, soft, tube, turnFromTo, UP, v3, type Colour, type Geometry, type Sheet, type V } from '../../shape';
import type { Broadleaf, Lod } from '../../trees';
import { bend, lerp, rounded, stem, twig } from './parts';

// A plume of leaves along z from 0 to 1.
const PLUME: [number, number][] = [[0, 0], [0.7, 0.08], [1, 0.3], [0.92, 0.6], [0.72, 0.88], [0, 1]];

/**
 * The crown of a `rods` shrub: the rods fan from the stool, the outer ones lower and leaning further, each with
 * two side shoots. Leafy, a plume wraps each rod's upper part and each shoot; bare, twigs stand along them.
 */
export function rods(spec: Broadleaf, lod: Lod, bare: boolean, foliage: (p: V, n: V) => Colour, [, tip]: Colour[], [catkin, share]: [Colour?, number?], rand: () => number, seed: number, withHeads: boolean): Geometry[] {
    const [cz, rx, ry, rz] = spec.crown, n = spec.rods!, sides = [4, 3, 0][lod];
    const foot = hex(spec.bark[0]), wood = hex(spec.twigs[1]), young = hex(spec.twigs[2]);
    const bark = (p: V) => foot.clone().lerp(wood, smooth(0, 0.2, p.z)).lerp(young, smooth(0.3, 0.9, p.z));
    const leaf = (tone: number) => (p: V, normal: V) => foliage(v3(p.x / rx, p.y / ry, (p.z - cz) / rz), normal).multiplyScalar(tone);
    const parts: Geometry[] = [], twigs = sheet();
    let kept = 0;
    // A plume around `path`, `width` across, in `tufts` tufts that stand out to one side after the other: a leafy
    // wand, not a blade. A far one has fewer corners, and so has a slim one.
    const slim = (spec.dense ?? 1) < 0.7;
    const plume = (path: (s: number) => V, from: number, width: number, k: number, tufts: number) => {
        const geo = soft(lathe(PLUME, [slim ? 4 : 6, slim ? 3 : 4, 3][lod], [2 * tufts, 4, 2][lod]), 0.1, 4, seed + k);
        const wave = (q: number) => 2 * Math.PI * (tufts / 2) * q + k;
        bend(geo, path, from, 1.02, (x, y, q) => [width * ((1 + 0.3 * Math.cos(2 * wave(q))) * x + 0.35 * Math.sin(wave(q))), width * ((1 + 0.3 * Math.cos(2 * wave(q))) * y + 0.35 * Math.cos(wave(q)))]);
        parts.push(paint(rounded(geo, 0.5), leaf(0.9 + 0.2 * rand())));
    };
    // Twigs stand along a bare rod, close to it and upward.
    const bristle = (path: (s: number) => V, from: number, count: number, length: number) => {
        for (let j = 0; j < count; j++) {
            const s = lerp(from, 1, (j + rand()) / count), root = path(s), along = path(s + 0.01).sub(path(s - 0.01)).normalize();
            const way = v3(rand() - 0.5, rand() - 0.5, 0).multiplyScalar(1.1).add(along).normalize();
            twig(twigs, root, root.clone().addScaledVector(way, length * (0.6 + 0.8 * rand())), [wood, tip], rand, lod, kept++);
        }
    };
    for (let i = 0; i < n; i++) {
        const ring = Math.sqrt((i + 0.5) / n), a = i * GOLDEN;
        const height = (cz + rz) * lerp(1, 0.55, ring ** 1.5) * (0.92 + 0.16 * rand());
        const reach = ring * (0.85 + 0.3 * rand()), start = 1.2 * spec.trunk * ring;
        // A rod arches out of the stool, then stands, so the shrub is round; its tip nods out.
        const path = (s: number) => {
            const k = 0.6 * (1 - (1 - Math.min(1, Math.max(0, s))) ** 2) + 0.4 * s ** 3;
            return v3(Math.cos(a) * lerp(start, reach * rx, k), Math.sin(a) * lerp(start, reach * ry, k), lerp(0.5 * spec.fork, height, s));
        };
        stem(parts, twigs, [0, 0.2, 0.5, 1].map(path), 0.4 * spec.trunk, 0.1 * spec.trunk, bark, sides, i);
        const width = 0.034 * (spec.dense ?? 1) * (0.8 + 0.4 * rand());
        if (bare) bristle(path, 0.35, 8, 0.09);
        else plume(path, 0.3 + 0.12 * rand(), width, 5 * i, 5);
        for (let b = 0; b < 2; b++) {
            const from = path(0.25 + 0.5 * rand()), turn = a + (b % 2 ? 1 : -1) * (0.5 + 1.2 * rand()), length = height * (0.16 + 0.16 * rand());
            const out = v3(0.55 * Math.cos(turn), 0.55 * Math.sin(turn), 1).normalize();
            const shoot = (s: number) => from.clone().addScaledVector(out, length * s).addScaledVector(UP, 0.15 * length * s * s);
            if (bare) {
                stem(parts, twigs, (lod < 2 ? [0, 0.5, 1] : [0, 1]).map(shoot), 0.15 * spec.trunk, 0.05 * spec.trunk, () => young, lod ? 0 : 3, i + b);
                bristle(shoot, 0.2, 4, 0.07);
            } else plume(shoot, 0, 0.8 * width, 5 * i + 1 + b, 2);
        }
        // A catkin stands on the plume, up and out: three a rod, or the season's share of the shrub's heads.
        const { count, width: across, height: tall } = spec.heads ?? { count: 0, width: 0.022, height: 0 };
        const catkins = !catkin || bare || lod ? 0 : spec.heads ? Math.round((count * share!) / n + rand() - 0.5) : 3;
        for (let c = 0; c < catkins; c++) {
            const s = 0.4 + 0.5 * rand(), turn = 2 * Math.PI * rand(), out = v3(Math.cos(turn), Math.sin(turn), 0);
            // Without its heads the shrub draws as many random numbers, so its rods stay.
            if (spec.heads && !withHeads) continue;
            const geo = paint(soft(lathe([[0, 0], [1, 0.4], [0, 1]], 4, 2)), catkin!);
            parts.push(place(geo, { at: path(s).addScaledVector(out, width), turn: turnFromTo(UP, out.setZ(1.2).normalize()), scale: [0.5 * across, 0.5 * across, spec.heads ? across * tall : 0.065] }));
        }
    }
    return [...parts, twigs.build()];
}

/**
 * Fronds arch out of a knob of leaf bases, the young ones upright and the old ones hanging. Near
 * the eye each leaflet is one card, so a frond reads as a comb: 48 fronds of 11 pairs. Further off
 * a frond is one strip over the comb's outline, which keeps the crown's mass.
 */
export function palm(spec: Broadleaf, lod: Lod, [deep, mid, tip]: Colour[], rand: () => number): Geometry[] {
    const [cz, length] = spec.crown;
    const count = [spec.fronds!, 36, 16][lod];
    const segments = [11, 4, 2][lod];
    const base = v3(0, 0, spec.fork);
    const light = domed(v3(0, 0, cz), v3(1, 1, 1), 0.3);
    const cards = sheet();
    for (let f = 0; f < count; f++) {
        const age = (f + 0.5) / count;
        const rise = lerp(1.3, -0.6, age) + 0.15 * (rand() - 0.5);
        const a = f * GOLDEN;
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const side = v3(-Math.sin(a), Math.cos(a), 0);
        const reach = length * (0.85 + 0.25 * rand());
        // Even the upright fronds arch over, so the crown reads as a fountain, not a spray of spikes.
        const sag = 0.25 + 0.25 * Math.cos(rise);
        const along = (s: number) =>
            base.clone().addScaledVector(out, Math.cos(rise) * s * reach).setZ(spec.fork + (Math.sin(rise) * s - sag * s * s) * reach);
        const width = (s: number) => 0.22 * reach * Math.sin(Math.PI * Math.min(1, s + 0.12)) ** 0.7;
        // Old fronds yellow towards the tips.
        const lit = tip.clone().lerp(hex('#c8b860'), 0.6 * smooth(0.6, 1, age));
        const root = (s: number) => deep.clone().lerp(mid, s);
        const rim = (s: number) => mid.clone().lerp(lit, smooth(0.2, 0.9, s));
        for (let j = 0; j < segments; j++) {
            const s = 0.12 + (0.86 * j) / segments;
            // Leaflets leave gaps between them; strip segments meet.
            const t = s + (lod ? 0.86 : 0.8) / segments;
            const [p, q] = [along(s), along(t)];
            const forward = q.clone().sub(p).normalize();
            // Where a leaflet from c ends: out, forward and up.
            const leaflet = (c: V, s: number, sign: number) =>
                c.clone().addScaledVector(side, sign * width(s)).addScaledVector(forward, 0.4 * width(s)).add(v3(0, 0, 0.3 * width(s)));
            // Darker than the rim, as a comb's roots and the shade between its leaflets are.
            const rib = (s: number) => root(s).lerp(rim(s), 0.25);
            for (const sign of [-1, 1]) {
                const ep = leaflet(p, s, sign);
                if (lod === 0) cards.add([p, q, ep], [root(s), root(s), rim(s)], light);
                else {
                    cards.add([p, q, ep], [rib(s), rib(t), rim(s)], light);
                    // The tip narrows to a point.
                    if (j < segments - 1) cards.add([q, leaflet(q, t, sign), ep], [rib(t), rim(t), rim(s)], light);
                }
            }
        }
    }
    // Leaf bases far off are as green as the fronds around them, lest they read as a nut.
    const husk = hex('#6a5a3c').lerp(deep, lod ? 0.6 : 0);
    // The knob's top is as green as the young fronds that leave it.
    const knob = paint(place(soft(lump((lod ? 1.3 : 1.7) * spec.trunk, lod ? 0 : 1), 0.1 * spec.trunk, 30), { at: base }), (p) => husk.clone().lerp(mid, smooth(0, spec.trunk, p.z - spec.fork)));
    return [cards.build(), knob];
}

/**
 * Limbs fork `branching` ways at each level, rising and spreading so the rosettes of sword
 * leaves at their ends make an umbrella. Further off the rosettes have fewer, wider blades; a far
 * tree keeps two levels, each limb ending in a ragged bowl where its rosettes would be.
 */
export function dragon(spec: Broadleaf, lod: Lod, [deep, mid, tip]: Colour[], rand: () => number): Geometry[] {
    const [cz, rx, , rz] = spec.crown;
    const levels = spec.branching!.slice(0, lod < 2 ? 3 : 2);
    const [foot, bole] = spec.bark.map(hex);
    const bark = (p: V) => foot.clone().lerp(bole, smooth(0, spec.fork, p.z));
    const size = 0.2;
    const fork = v3(0, 0, spec.fork);
    // A collar where the limbs leave the trunk, so the trunk swells into them.
    const parts = lod < 2 ? [paint(place(soft(lump(1.1 * spec.trunk, lod ? 0 : 1), 0.08 * spec.trunk, 30), { at: fork }), bark)] : [];
    const cards = sheet();
    const grow = (from: V, a0: number, a1: number, level: number, r: number) => {
        const m = levels[level];
        for (let c = 0; c < m; c++) {
            const a = a0 + ((c + 0.5 + 0.4 * (rand() - 0.5)) / m) * (a1 - a0);
            const f = (level + 1) / levels.length;
            const d = rx * f ** 0.8 * (0.75 + 0.35 * rand());
            const z = spec.fork + (cz + rz * Math.sqrt(Math.max(0, 1 - (d / rx) ** 2)) - size - spec.fork) * f ** 0.45;
            const to = v3(d * Math.cos(a), d * Math.sin(a), z);
            // A limb leaves level and turns up under its fork, as thick at its end as the limbs that leave it.
            const knee = from.clone().lerp(to, 0.55).setZ(lerp(from.z, to.z, 0.35));
            parts.push(tube(lod ? [from, to] : [from, knee, to], r, 0.8 * r, [6, 4, 3][lod], bark));
            if (level + 1 < levels.length) grow(to, a0 + (c * (a1 - a0)) / m, a0 + ((c + 1) * (a1 - a0)) / m, level + 1, 0.75 * r);
            else if (lod === 2) bowl(cards, to, a, 0.42 * rx, size, [deep, mid, tip]);
            else {
                rosette(cards, to, size, lod ? 11 : 32, lod ? 0.3 : 0.14, [deep, mid, tip], rand);
                // A heart fills the rosette between its blades.
                if (!lod) parts.push(paint(place(soft(lump(0.38 * size, 0)), { at: to.clone().setZ(to.z + 0.15 * size) }), deep.clone().lerp(mid, 0.5)));
            }
        }
    };
    grow(fork, 0, 2 * Math.PI, 0, 0.8 * spec.trunk);
    return [...parts, cards.build()];
}

/**
 * A far group of rosettes: a bowl of 8 triangles from a limb's end, out towards `heading`, its
 * rim ragged so its top edge reads as blades.
 */
function bowl(cards: Sheet, at: V, heading: number, radius: number, height: number, [deep, mid, tip]: Colour[]): void {
    const rim = at.clone().add(v3(0.5 * radius * Math.cos(heading), 0.5 * radius * Math.sin(heading), height));
    const light = domed(rim.clone().setZ(rim.z - 2 * height), v3(1, 1, 1), 0.3);
    const point = (k: number) => {
        const a = heading + (k / 8) * 2 * Math.PI;
        const out = k % 2 ? 0.7 : 1;
        return rim.clone().add(v3(out * radius * Math.cos(a), out * radius * Math.sin(a), (out - 0.75) * height));
    };
    const [spike, notch] = [mid.clone().lerp(tip, 0.15), deep.clone().lerp(mid, 0.3)];
    for (let k = 0; k < 8; k++) cards.add([at, point(k), point(k + 1)], [deep, k % 2 ? notch : spike, k % 2 ? spike : notch], light);
}

/** Stiff sword leaves up and out from one point, only the outer ring drooping: `count` of them, each `width` of its length wide. */
function rosette(cards: Sheet, at: V, size: number, count: number, width: number, [deep, mid, tip]: Colour[], rand: () => number): void {
    const light = domed(at, v3(1, 1, 1), 0.3);
    for (let j = 0; j < count; j++) {
        const a = j * GOLDEN + 0.5 * rand();
        const t = (j + 0.5) / count;
        const polar = 1.3 * Math.sqrt(t) + 0.45 * smooth(0.7, 1, t);
        const reach = size * (0.8 + 0.35 * rand());
        const end = at.clone().add(v3(Math.sin(polar) * Math.cos(a), Math.sin(polar) * Math.sin(a), Math.cos(polar)).multiplyScalar(reach));
        const side = v3(-Math.sin(a), Math.cos(a), 0).multiplyScalar(width * reach);
        cards.add([at.clone().sub(side), at.clone().add(side), end], [deep, deep, mid.clone().lerp(tip, smooth(0.5, 0, polar) + 0.3 * rand())], light);
    }
}
