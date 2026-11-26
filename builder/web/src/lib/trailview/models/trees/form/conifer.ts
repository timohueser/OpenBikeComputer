// A conifer's crown: stacked tiers, whorls of tufts, a column or separate boughs.

import type { Season } from '../../../rules/regions';
import { computeNormals, GOLDEN, hash, hex, lathe, limb, lump, merge, noise, paint, place, ramp, random, revolve, sheet, smooth, soft, v3, type Colour, type Geometry, type V, type Vec } from '../../shape';
import type { Built, Conifer, Lod } from '../../trees';
import { trunk } from './bark';
import { bend, berries, bough, DETAIL, hang, lerp, lowest, rounded, stem, twig, TWIG_WIDTH, TWIGS, upright, vertices } from './parts';

// One tier of branches, radius 1 and height 1: out along the underside to the rim, then up to the tip.
const TIER: [number, number][] = [[0, -0.04], [0.45, -0.08], [0.86, -0.12], [1, -0.04], [0.97, 0.05], [0.78, 0.2], [0.42, 0.52], [0.12, 0.87], [0, 1]];

/** Droops a tier's rim into branch tips. Whole lobes keep the lathe seam closed. */
function scallop(geo: Geometry, lobes: number, droop: number, seed: number): Geometry {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) {
        const [x, y, z] = [p[i], p[i + 1], p[i + 2]];
        const rim = smooth(0.55, 0.95, Math.hypot(x, y));
        const tip = 0.5 + 0.5 * Math.cos(Math.atan2(y, x) * lobes + seed);
        const grow = 1 + 0.08 * rim * (tip - 0.4);
        p.splice(i, 3, x * grow, y * grow, z - droop * rim * tip);
    }
    return geo;
}

/** Stacked tiers along the crown; fewer, taller tiers keep the same outline at a lower detail. */
export function conifer(spec: Conifer, season: Season, lod: Lod, seed: number): Built {
    const { sides } = DETAIL[lod];
    const rand = random(seed);
    const own = spec.needles[season];
    const bare = own === 'bare';
    // A far tufted pine keeps its whorls, as its tiers are layers apart, not one cone; so does a bare tree.
    const tiers = spec.whorls ?? DETAIL[bare || spec.tufts ? 0 : lod].tiers, rows = spec.whorls ? Math.round((DETAIL[lod].rows * DETAIL[lod].tiers) / tiers) : DETAIL[lod].rows;
    const [deep, mid, tip] = ramp(bare ? spec.twigs! : own);
    const under = spec.under ? hex(spec.under) : deep;
    const needles = (p: V, n: V) =>
        deep
            .clone()
            .lerp(mid, smooth(-0.2, 0.6, n.z))
            .lerp(under, 0.5 * smooth(-0.4, -0.9, n.z))
            .lerp(tip, 0.6 * smooth(0.35, 1, p.z) * smooth(0, 0.5, n.z));
    // A tier with more tips than corners would droop on one side only.
    const lobes = Math.min(spec.lobes, Math.floor(sides / 2));
    const step = (1 - spec.base) / (tiers - 1 + spec.overlap);
    const height = spec.overlap * step;
    const sways = spec.boughs ? boughs(spec, rand) : [];
    // A column hides its trunk but for the foot; boughs show theirs up to the leader; a nest covers its trunk's top.
    const parts = [trunk(spec, bare ? 0.97 : spec.column ? spec.base + 0.1 : spec.boughs ? 0.88 : 1 - height - (spec.nest ? 0.03 : 0), lod, seed)];
    const twigs = sheet();
    let kept = 0;
    const bole = hex(spec.bark[1]);
    if (spec.column) parts.push(...column(spec, needles, lod, seed));
    for (const [k, { path, r }] of sways.entries()) {
        if (bare) {
            stem(parts, twigs, (lod < 2 ? [0, 0.33, 0.66, 1] : [0, 1]).map(path), spec.trunk * 0.3, spec.trunk * 0.06, () => mid, lod ? 0 : 4, k);
            // Twigs hang from the bough in a curtain.
            for (let j = 0; j < CURTAIN; j++) {
                const root = path(0.15 + (0.9 * (j + rand())) / CURTAIN), a = 2 * Math.PI * rand(), d = 0.1 * r * rand();
                const end = root.clone().add(v3(d * Math.cos(a), d * Math.sin(a), -r * (0.25 + 0.3 * rand())));
                // Wider than a broadleaf's twig near the eye: a larch's hanging twigs are knobbly with spurs.
                const { thin } = DETAIL[lod], face = rand() < 0.5 ? 1 : -1;
                if (kept++ % thin === 0) upright(twigs, root, end.setZ(Math.max(end.z, 0.01)), (lod < 2 ? 2 : 1) * thin * TWIG_WIDTH, [mid, tip], face);
            }
            continue;
        }
        // A far tree keeps every other bough, its spray wide enough to fill the cone's outline.
        if (lod === 2 && k % 2) continue;
        const wide = lod === 2 ? 0.75 * r + 0.012 : 0.5 * r;
        // A leaf-shaped spray over the bough, from the trunk; its underside hangs in a curtain of two or three points.
        const spray = soft(lathe(SPRAY, [6, 4, 4][lod], [7, 4, 2][lod]), 0.05, 4, seed + k);
        const hang = (q: number) => 2.4 + 1.6 * Math.cos(2 * Math.PI * 2.5 * q + k);
        parts.push(paint(rounded(bend(spray, path, 0.02, 1, (x, y, q) => [wide * x, 0.1 * r * y * (y > 0 ? (lod === 2 ? 3 : 1.5) : hang(q))]), 0.5), needles));
    }
    // The leader stands over the top boughs.
    if (sways.length && !bare) parts.push(place(paint(soft(lathe([[0, 0], [1, 0.2], [0.55, 0.6], [0, 1]], sides >> 1, 4)), needles), { at: [0, 0, 0.82], scale: [0.03, 0.03, 0.18] }));
    const lift = spec.lift ?? 0, rise = Math.min(1, lift);
    // The needles on the end of a branch that leaves the trunk towards `a`. A tuft that droops is a mop, a dome over a
    // fringe that hangs in points; another is a ball, longer along a branch that rises and rounder. Far off both are
    // balls of 20 triangles.
    const tuft = (at: Vec, size: number, k: number, a = 0) => {
        const mop = spec.droop > 0;
        const geo = mop && !lod ? soft(scallop(lathe(MOP, 8, 6), 4, spec.droop, seed + k), 0.05, 3, seed + k) : hang(soft(lump(1, lod ? 0 : 1), 0.08, 3, seed + k), 1, spec.droop);
        const tone = 0.9 + 0.2 * hash(v3(...at), seed), scale: Vec = mop ? [size, size, size] : [(1 + 0.3 * rise) * size, size, lerp(0.5, 0.8, rise) * size];
        const tilted = place(paint(geo, (p, n) => needles(p, n).multiplyScalar(tone)), { rot: [0, mop ? 0 : -Math.atan(lift), 0], scale });
        return place(tilted, { at, rot: [0, 0, a] });
    };
    // Far off a whorl's tufts merge into one layer of radius 1 with a point for each tuft, its rim as high as the branches rise.
    const layer = () => {
        const n = spec.lobes, thick = lerp(0.5, 0.8, rise) * spec.tufts!, geo = soft(lathe([[0, -thick], [1, 0], [0, thick]], 2 * n, 2)), p = geo.position;
        for (let i = 0; i < p.length; i += 3) {
            const d = Math.hypot(p[i], p[i + 1]), dent = Math.round((Math.atan2(p[i + 1], p[i]) * n) / Math.PI) % 2 ? Math.min(0.9, 0.1 + spec.tufts!) : 1;
            // The rim of mops hangs; that of balls stands as high as their branches rise.
            p.splice(i, 3, p[i] * dent, p[i + 1] * dent, p[i + 2] + (spec.droop > 0 ? -0.5 * spec.droop : lift * (1 - 0.8 * spec.tufts!)) * d * dent);
        }
        return paint(computeNormals(geo), needles);
    };
    for (let i = 0; i < (spec.column || spec.boughs ? 0 : tiers); i++) {
        const z = spec.base + i * step;
        const r = spec.radius * spec.shape((z - spec.base) / (1 - spec.base)) * (1 + spec.jitter * (2 * rand() - 1));
        const shift = spec.jitter * r * (2 * rand() - 1);
        const spin = rand() * Math.PI * 2;
        if (bare) {
            // A whorl of bare branches in the place of the tier, drooping as far, twigs hanging from them.
            for (let k = 0; k < 5; k++) {
                const a = spin + (k / 5) * 2 * Math.PI;
                const from = v3(0, 0, z + 0.3 * height);
                const end = v3(shift + r * Math.cos(a), r * Math.sin(a), z - spec.droop * height);
                // Branches thinner than a broadleaf's limbs go flat a level sooner.
                bough(parts, twigs, from, end, spec.trunk * 0.3, () => mid, DETAIL[lod + 1]?.limbs ?? 0, 5 * i + k, spec.trunk * 0.1);
                // A conifer's crown is narrow, so each branch has a thirtieth of a broadleaf crown's twigs.
                for (let j = 0; j < TWIGS / 30; j++) {
                    const root = from.clone().lerp(end, 0.3 + 0.7 * rand());
                    const out = v3(rand() - 0.5, rand() - 0.5, rand() - 0.5).normalize().add(v3(Math.cos(a), Math.sin(a), -0.6)).normalize();
                    twig(twigs, root, root.clone().addScaledVector(out, 0.4 * r), [mid, tip], rand, lod, kept++);
                }
            }
            continue;
        }
        if (spec.tufts && lod === 2) {
            parts.push(place(layer(), { at: [shift, 0, z + 0.2 * height + (spec.droop > 0 ? lift * (1.25 - spec.tufts) * r : 0)], rot: [0, 0, spin], scale: [1.25 * r, 1.25 * r, 1.25 * r] }));
            continue;
        }
        if (spec.tufts) {
            const n = spec.lobes;
            for (let k = 0; k < n; k++) {
                const a = spin + ((k + 0.5 * rand()) / n) * 2 * Math.PI;
                // A wider tuft stands nearer the trunk, so the whorl is as wide.
                const out = (1.25 - spec.tufts) * r;
                // Tufts of one whorl stand a little higher or lower and are larger or smaller, by `jitter`.
                const end: Vec = [shift + out * Math.cos(a), out * Math.sin(a), z + 0.2 * height + lift * out + spec.jitter * step * (rand() - 0.5)];
                parts.push(limb([0, 0, z], end, spec.trunk * 0.3, bole, 0, 4));
                parts.push(tuft(end, spec.tufts * r * (1 + spec.jitter * (rand() - 0.5)), i * n + k, a));
            }
            continue;
        }
        const geo = soft(scallop(lathe(TIER, sides, rows), lobes, spec.droop, seed + i), 0.02, 4, seed + i);
        parts.push(place(paint(geo, needles), { at: [shift, shift * 0.5, z], rot: [0, 0, spin], scale: [r, r, spec.nest && i === tiers - 1 ? 0.4 * height : height] }));
    }
    if (spec.tufts && !bare) parts.push(tuft([0, 0, 1 - 0.5 * height], 0.35 * spec.radius, -1));
    // No berries near the top, where the crown is too thin to hold them.
    if (spec.berries && !lod && !bare) parts.push(...berries(parts.slice(1), spec.berries, rand, p => p.z > lerp(spec.base, 1, 0.7)));
    const crown = parts.slice(1);
    // Sprouts are too small to see beyond the near trees.
    for (let k = 0; !bare && k < (spec.sprouts ?? 0) >> (2 * lod); k++) {
        const a = rand() * 2 * Math.PI;
        const size = spec.trunk * (0.8 + 0.6 * rand());
        const at: Vec = [1.2 * spec.trunk * Math.cos(a), 1.2 * spec.trunk * Math.sin(a), spec.base * (0.25 + 0.7 * rand())];
        parts.push(tuft(at, size, 100 + k));
    }
    if (bare) {
        const built = twigs.build();
        parts.push(built);
        crown.push(built);
    }
    const geo = merge(parts);
    if (spec.nod) nod(geo, spec.nod);
    return { geo, trunk: vertices(parts.slice(0, 1)), crown: lowest(crown) };
}

// A mop of needles, radius 1: a dome over a rim that `scallop` droops.
const MOP: [number, number][] = [[0, -0.2], [0.5, -0.3], [0.9, -0.35], [1, -0.1], [0.9, 0.3], [0.55, 0.7], [0, 0.85]];

/** Twigs that hang from a bare bough of a near tree. */
const CURTAIN = 16;

// A spray along z from 0 to 1, radius 1: broad near its root, with a round tip.
const SPRAY: [number, number][] = [[0, 0], [0.75, 0.12], [1, 0.42], [0.88, 0.76], [0.45, 0.96], [0, 1]];

/**
 * A conifer's boughs, lowest first: each one's length and its path from the trunk (s = 0) to its tip (s = 1),
 * which sags and turns up again. The upper boughs rise, as young ones do.
 */
function boughs(spec: Conifer, rand: () => number): { path: (s: number) => V; r: number }[] {
    return Array.from({ length: spec.boughs! }, (_, i) => {
        const t = (i + 0.5) / spec.boughs!, a = i * GOLDEN + 0.4 * rand(), z = lerp(spec.base, 0.9, t);
        const r = spec.radius * spec.shape(t) * (1 + spec.jitter * (rand() - 0.5));
        return { r, path: (s: number) => v3(r * s * Math.cos(a), r * s * Math.sin(a), z + r * (spec.droop * (1.6 * s ** 3 - 2 * s) + 1.2 * t ** 3 * s)) };
    });
}

/**
 * The crown of a `column` conifer, and its spires: each a flame of the same outline that leans out of the column's side
 * to a lower top. A swell is lighter and a hollow darker, so the lumps show in flat light.
 */
function column(spec: Conifer, needles: (p: V, n: V) => Colour, lod: Lod, seed: number): Geometry[] {
    const spires = spec.spires ?? 0;
    const flame = (k: number, radius: number, top: number, rows: number, sides: number, lean: V) => {
        if (spec.knobs) {
            // Corners as far apart around the column as up it, and whole knobs around it.
            const around = (2 * Math.PI * 0.7 * radius) / (top - spec.base), quads = rows * sides;
            [rows, sides] = [Math.round(Math.sqrt(quads / around)), 4 * Math.max(1, Math.round(Math.sqrt(quads * around) / 4))];
        }
        const outline = Array.from({ length: rows + 1 }, (_, j): [number, number] => [radius * spec.shape(j / rows), lerp(spec.base, top, j / rows)]);
        const geo = soft(revolve([[0, spec.base], ...outline], sides)), p = geo.position, swell: number[] = [];
        for (let i = 0; i < p.length; i += 3) {
            const out = (p[i + 2] - spec.base) / (top - spec.base);
            if (spec.knobs) {
                // A knob swells from its edges to its middle and stands out along the skin's normal, some further, so the
                // top is in knobs too. The knobs of one upright row start a quarter, a half or three quarters of a knob higher
                // than those of the next.
                const u = ((Math.atan2(p[i + 1], p[i]) / (2 * Math.PI) + 1) * sides) / 4 + 0.01, row = Math.floor(u) % (sides / 4);
                const v = (out * rows) / 4 + Math.floor(4 * hash(v3(row, k, 0), seed)) / 4;
                swell.push(Math.sin(Math.PI * (u % 1)) * Math.sin(Math.PI * (v % 1)) * (0.5 + 0.5 * hash(v3(row, Math.floor(v), k), seed)));
                const deep = spec.jitter * radius * swell[i / 3];
                p.splice(i, 3, p[i] + deep * geo.normal[i] + lean.x * out, p[i + 1] + deep * geo.normal[i + 1] + lean.y * out, p[i + 2] + deep * geo.normal[i + 2]);
                continue;
            }
            // Lumps taller than wide, as sprays that stand upright.
            swell.push(noise(v3(p[i], p[i + 1], 0.5 * p[i + 2]), 2.6 / radius, seed + k));
            p[i] = p[i] * (1 + spec.jitter * swell[i / 3]) + lean.x * out;
            p[i + 1] = p[i + 1] * (1 + spec.jitter * swell[i / 3]) + lean.y * out;
        }
        let v = 0;
        return paint(computeNormals(geo), (q, n) => needles(q, n).multiplyScalar(1 + 0.4 * swell[v++]));
    };
    // A column with spires has fewer corners, as they share its triangles.
    const main = flame(0, spec.radius, 1, spires ? [36, 24, 10][lod] : [60, 24, 10][lod], spires && !lod ? 16 : DETAIL[lod].sides, v3());
    return [main, ...Array.from({ length: spires }, (_, k) => {
        const a = k * GOLDEN + seed, top = lerp(spec.base, 1, 0.6 + (0.25 * k) / spires);
        return flame(k + 1, 0.7 * spec.radius, top, [20, 10, 5][lod], [10, 6, 4][lod], v3(Math.cos(a), Math.sin(a), 0).multiplyScalar(1.1 * spec.radius));
    })];
}

/** Bends the top of the leader over towards +x. */
function nod(geo: Geometry, amount: number): void {
    const p = geo.position, from = 1 - 3 * amount;
    for (let i = 0; i < p.length; i += 3) {
        const k = Math.max(0, p[i + 2] - from) / (1 - from);
        p[i] += amount * k * k;
        p[i + 2] -= 0.6 * amount * k * k * k;
    }
    computeNormals(geo);
}
