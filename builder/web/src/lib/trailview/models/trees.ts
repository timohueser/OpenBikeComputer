// The tree builder, the prototype's src/models/trees.ts without three.js: a species file in trees/
// says what a tree is, and this draws it at three levels of detail. A tree is 1 tall with its trunk
// base at the origin and z up; the instance shader scales it by the tree's height. The season changes
// vertex colours, and a bare tree's crown.

import type { Season } from '../rules/regions';
import {
    computeNormals, domed, GOLDEN, hash, hex, lathe, limb, lump, merge, noise, nonIndexed, paint, place, ramp, random, revolve, sheet, smooth, soft, tube, turnFromTo, unit, UP, v3,
    type Colour, type Geometry, type Ramp, type Sheet, type V, type Vec,
} from './shape';

/** 0 the most detail; each level has fewer triangles. */
export type Lod = 0 | 1 | 2;

const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));

/**
 * Segments around a part, spline rows per tier, conifer tiers and broadleaf crown lumps. A trunk
 * has its own sides and rows per profile span. A bare crown's limbs have `limbs` sides, or are flat
 * at 0, and it keeps every `thin`-th twig of the nearest level.
 */
const DETAIL = [
    { sides: 22, rows: 10, tiers: 7, trunk: [10, 2], lumps: 9, ico: 3, limbs: 6, thin: 1 },
    { sides: 10, rows: 6, tiers: 5, trunk: [6, 1], lumps: 6, ico: 1, limbs: 4, thin: 3 },
    { sides: 6, rows: 3, tiers: 3, trunk: [4, 1], lumps: 5, ico: 0, limbs: 0, thin: 6 },
] as const;
/**
 * A bare crown has TWIGS twigs at the nearest level, each TWIG_WIDTH wide at its root in tree
 * heights, a conifer's a thirtieth as many per branch. Every level grows the limbs, whorls and twigs
 * of the nearest; a farther level keeps every `thin`-th twig, `thin` times as wide, so the crown keeps
 * its outline and covers as much.
 */
const TWIGS = 400, TWIG_WIDTH = 0.004;
/** The branches that a bare broadleaf limb forks into. */
const BRANCHES = 3;

interface Bark {
    /** Trunk radius at the ground, and the root flare in trunk radii. */
    trunk: number;
    flare: number;
    /** The trunk's radius at its top, in ground radii; 0.6 by default. */
    neck?: number;
    /** Colour at the ground and higher up. */
    bark: [string, string];
    /** Where the ground colour turns into the colour higher up, in shares of the trunk; [0, 0.5] by default. */
    fade?: [number, number];
    /** Dark marks on pale bark, as on birch: their colour and share of the bark. */
    marks?: [string, number];
    /** Diamond leaf scars, as on a palm: how many around the trunk. */
    scars?: number;
}

interface Base extends Bark {
    name: string;
    latin: string;
    /** Placed at the heading it is built in, as a tree the wind bends must be; others turn at random. */
    keepsHeading?: true;
}

interface Conifer extends Base {
    form: 'conifer';
    /** Crown base height and crown radius there, in tree heights. */
    base: number;
    radius: number;
    /** The crown's radius from 1 at its base (t = 0) to its top (t = 1). */
    shape: (t: number) => number;
    /** Tier height in tier spacings. Near 1 the trunk shows between the tiers. */
    overlap: number;
    /** How far branch tips hang, in tier heights, and how many tips a tier has. */
    droop: number;
    lobes: number;
    /** Tier radius and position vary by this share from tier to tier. */
    jitter: number;
    needles: Record<Season, Ramp | 'bare'>;
    /** The colour of a bare conifer's branches and twigs. */
    twigs?: Ramp;
    /** Pale needle undersides, as on silver fir. */
    under?: string;
    /** How far the leader nods over towards +x, in tree heights. */
    nod?: number;
    /** Each tier a whorl of branches ending in drooping tufts, with sky between, as on a Canary pine. */
    tufts?: true;
    /** Needle tufts sprouting from the bare trunk: how many a near tree has. */
    sprouts?: number;
    /** One smooth column over the crown's outline in soft upright lumps, `jitter` deep, as a cypress: no tiers. */
    column?: true;
    /**
     * Separate boughs in place of tiers, with sky between: each sags and turns up at its tip under a hanging
     * curtain of needles, as on a larch. How many a tree has.
     */
    boughs?: number;
}

interface Broadleaf extends Base {
    form: 'broadleaf';
    /** Where the trunk divides. A bush has no trunk. */
    fork: number;
    /** Crown centre height and half-sizes along x, y and z. */
    crown: [number, number, number, number];
    /** How far the top is blown over towards +x, in tree heights. */
    sweep?: number;
    /** An open crown of separate flat puffs on lifted branches, as on a pine, instead of a dome. */
    open?: true;
    /** Limbs leave the trunk at staggered heights below their lumps, as on an old pine, not from one fork. */
    stagger?: true;
    /** Puffs are flat-topped pads, as on a Scots pine; a near tree has three more. */
    pads?: true;
    /** Lumps hang into points this far, in lump radii, as the twigs of a weeping birch. */
    weep?: number;
    /** Trunks leaning out from one foot. */
    stems?: number;
    /** Each limb leaves the stem on its side of the crown; without it every limb leaves the middle between the stems. */
    leaders?: true;
    /** The trunk bends this far from side to side on its way up, in tree heights, and the limbs bow up into their puffs. */
    crook?: number;
    /**
     * A shrub of slender rods that rise from one stool in a vase, each in a narrow plume of leaves, as a
     * shrub willow: how many it has. Its flecks are catkins that stand on the plumes.
     */
    rods?: number;
    /** The flecks of a leafy crown are bunches that hang from it, as a hop hornbeam's fruit, not dots: how many a near tree has. */
    bunches?: number;
    /** A palm: this many fronds near the eye arch out of the trunk's top, as long as the crown is wide. */
    fronds?: number;
    /** A dragon tree: limbs fork this many ways at each level and end in rosettes of sword leaves. */
    branching?: number[];
    leaves: Record<Season, Ramp | 'bare'>;
    /** The colour of a bare crown's twigs. */
    twigs: Ramp;
    /**
     * Blossom, berries or flowers dotted over the crown, or on a bare crown whole twigs, as catkins or
     * dead leaves that hang on: colour and share, by season.
     */
    flecks?: Partial<Record<Season, [string, number]>>;
}

/** A species file in trees/. */
export type Tree = Conifer | Broadleaf;

/** A tree 1 tall: its first `trunk` vertices are its trunk, and `crown` is the lowest point of the rest in tree heights. */
export interface Grown extends Geometry { trunk: number; crown: number }

export function grow(spec: Tree, season: Season, lod: Lod, seed: number): Grown {
    const { geo, crown, trunk } = spec.form === 'conifer' ? conifer(spec, season, lod, seed) : broadleaf(spec, season, lod, seed);
    const top = Math.max(...geo.position.filter((_, i) => i % 3 === 2));
    return Object.assign(unit(geo), { trunk, crown: Math.max(0, crown / top) });
}

/** The lowest point of some parts. */
function lowest(parts: Geometry[]): number {
    let low = Infinity;
    for (const { position } of parts) for (let k = 2; k < position.length; k += 3) low = Math.min(low, position[k]);
    return low;
}

/** A tree and, as in `Grown`, its trunk's vertices first and its crown's lowest point, before `unit`. */
interface Built { geo: Geometry; trunk: number; crown: number }

const vertices = (parts: Geometry[]) => parts.reduce((sum, part) => sum + part.position.length / 3, 0);

/** Bands of marks up a near marked trunk; each holds up to two dashes. */
const BANDS = 18;

/** A trunk from the ground to `top`, darker at its foot, its bole in `spans` spans, which a trunk that bends needs. */
function trunk({ trunk: r, flare, neck = 0.6, bark, fade = [0, 0.5], marks, scars }: Bark, top: number, lod: Lod, seed: number, spans = 1): Geometry {
    if (scars && lod === 0) return scarred(r, flare, bark, scars, top);
    const [sides, perSpan] = DETAIL[lod].trunk;
    const [foot, bole] = bark.map(hex);
    const low = Math.min(6 * r, 0.5 * top);
    const at = (z: number): [number, number] => [r * lerp(1, neck, (z - low) / (top - low)), z];
    // A near marked trunk has a row at each edge of each band of marks, which are rows enough.
    const bands = marks && lod === 0 ? BANDS : 0;
    const centre = (b: number) => low + ((b + 0.5) / bands) * (top - low);
    const half = 0.3 * r;
    const profile: [number, number][] = [
        [0, 0],
        [r * flare, 0],
        // A far trunk's flare is one straight span.
        ...(lod < 2 ? [[(r * (1 + flare)) / 2, 2 * r] as [number, number]] : []),
        ...(bands ? Array.from({ length: bands }, (_, b) => [at(centre(b) - half), at(centre(b) + half)]).flat() : Array.from({ length: spans }, (_, k) => at(lerp(low, top, k / spans)))),
        at(top),
        [0, top + r],
    ];
    // The lathe samples its spline evenly per span, so whole rows per span keep every profile point.
    const geo = soft(lathe(profile, sides, (bands ? 1 : perSpan) * (profile.length - 1)), 0.075 * r, 1.4 / r, seed);
    // A far scarred trunk takes the mean shade of its plates and grooves.
    const colour = (p: V) => foot.clone().lerp(bole, smooth(fade[0] * top, fade[1] * top, p.z)).multiplyScalar(scars ? 0.8 : 1);
    if (!marks) return paint(geo, colour);
    const mark = hex(marks[0]);
    if (!bands) return paint(geo, (p) => colour(p).lerp(mark, 0.3 * marks[1]));
    // Dashes an eighth to a quarter of the way round.
    const rand = random(seed);
    const dashes = Array.from({ length: bands }, () => [0, 1].flatMap(() => (rand() < marks[1] ? [[2 * Math.PI * rand(), (Math.PI / 8) * (1 + rand())]] : [])));
    return stain(paint(geo, colour), mark, (c) => {
        const b = Math.round(((c.z - low) / (top - low)) * bands - 0.5);
        const a = Math.atan2(c.y, c.x);
        return Math.abs(c.z - centre(b)) < half && (dashes[b] ?? []).some(([to, width]) => Math.abs(((a - to + 3 * Math.PI) % (2 * Math.PI)) - Math.PI) < width);
    });
}

/** Paints whole faces whose centre `dark` picks, so marks keep the sharp edges vertex colours would blur. */
function stain(geo: Geometry, mark: Colour, dark: (centre: V) => boolean): Geometry {
    const flat = nonIndexed(geo), p = flat.position, c = v3();
    for (let i = 0; i < p.length / 3; i += 3) {
        c.set(p[3 * i] + p[3 * i + 3] + p[3 * i + 6], p[3 * i + 1] + p[3 * i + 4] + p[3 * i + 7], p[3 * i + 2] + p[3 * i + 5] + p[3 * i + 8]).divideScalar(3);
        if (dark(c)) for (let j = i; j < i + 3; j++) flat.color.splice(3 * j, 3, mark.r, mark.g, mark.b);
    }
    return flat;
}

/**
 * A trunk of diamond leaf scars in rings, each ring turned half a diamond. A diamond is four flat
 * faces around a raised centre, so the sun lights its upper half and the grooves stay dark:
 * 4 × n triangles a ring.
 */
function scarred(r: number, flare: number, bark: [string, string], n: number, top: number): Geometry {
    const [foot, bole] = bark.map(hex);
    // Rings half a diamond apart; a diamond is half as tall as it is wide.
    const h = (0.25 * 2 * Math.PI * r) / n;
    const corner = (i: number, k: number, raised = false) => {
        const z = i * h;
        // Scars run in a slow spiral, as the leaves grew.
        const a = (k / 2 + 0.03 * i) * ((2 * Math.PI) / n);
        const radius = r * (1 + (flare - 1) * (1 - smooth(0, 4 * r, z))) * (raised ? 1.15 : 1);
        return v3(radius * Math.cos(a), radius * Math.sin(a), z);
    };
    const cards = sheet();
    for (let i = 0; i * h < top; i++) {
        for (let k = i % 2; k < 2 * n; k += 2) {
            const c = corner(i, k, true);
            const ring = [corner(i, k + 1), corner(i + 1, k), corner(i, k - 1), corner(i - 1, k)];
            const plate = foot.clone().lerp(bole, smooth(0, 0.5 * top, c.z));
            const groove = plate.clone().multiplyScalar(0.55);
            for (let e = 0; e < 4; e++) cards.add([c, ring[e], ring[(e + 1) % 4]], [plate, groove, groove]);
        }
    }
    return cards.build();
}

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
function conifer(spec: Conifer, season: Season, lod: Lod, seed: number): Built {
    const { sides, rows } = DETAIL[lod];
    const rand = random(seed);
    const own = spec.needles[season];
    const bare = own === 'bare';
    // A far tufted pine keeps its whorls, as its tiers are layers apart, not one cone; a bare one all of them.
    const { tiers } = DETAIL[bare ? 0 : spec.tufts ? Math.min(lod, 1) : lod];
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
    // A column hides its trunk but for the foot; boughs show theirs up to the leader.
    const parts = [trunk(spec, bare ? 0.97 : spec.column ? spec.base + 0.1 : spec.boughs ? 0.88 : 1 - height, lod, seed)];
    const twigs = sheet();
    let kept = 0;
    const bole = hex(spec.bark[1]);
    if (spec.column) parts.push(column(spec, needles, lod, seed));
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
        // A leaf-shaped spray over the bough, from the trunk; its underside hangs in a curtain of two or three points.
        const spray = soft(lathe(SPRAY, [6, 4, 3][lod], [7, 4, 2][lod]), 0.05, 4, seed + k);
        const hang = (q: number) => 2.4 + 1.6 * Math.cos(2 * Math.PI * 2.5 * q + k);
        parts.push(paint(rounded(bend(spray, path, 0.02, 1, (x, y, q) => [0.5 * r * x, 0.1 * r * y * (y > 0 ? 1.5 : hang(q))]), 0.5), needles));
    }
    // The leader stands over the top boughs.
    if (sways.length && !bare) parts.push(place(paint(soft(lathe([[0, 0], [1, 0.2], [0.55, 0.6], [0, 1]], sides >> 1, 4)), needles), { at: [0, 0, 0.82], scale: [0.03, 0.03, 0.18] }));
    // A flat ball of needles drooping from a branch tip: 80 triangles near, 20, then a 12-triangle
    // double cone. A far tuft has too few corners to droop without turning into a point.
    const tuft = (at: Vec, size: number, k: number) => {
        const geo = lod === 2 ? soft(lathe([[0, -1], [1, 0], [0, 1]], 6, 2)) : soft(lump(1, lod ? 0 : 1), 0.08, 3, seed + k);
        if (lod < 2) hang(geo, 1, spec.droop);
        return place(paint(geo, needles), { at, scale: [size, size, 0.5 * size] });
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
            // Far off a whorl's tufts merge into one flat layer.
            parts.push(place(tuft([0, 0, 0], 1, i), { at: [shift, 0, z + 0.2 * height], rot: [0, 0, spin], scale: [1.25 * r, 1.25 * r, 0.85 * r] }));
            continue;
        }
        if (spec.tufts) {
            const n = spec.lobes;
            for (let k = 0; k < n; k++) {
                const a = spin + ((k + 0.5 * rand()) / n) * 2 * Math.PI;
                const end: Vec = [shift + 0.8 * r * Math.cos(a), 0.8 * r * Math.sin(a), z + 0.2 * height];
                parts.push(limb([0, 0, z], end, spec.trunk * 0.3, bole, 0, 4));
                parts.push(tuft(end, 0.45 * r * (1 + 0.2 * lod), i * n + k));
            }
            continue;
        }
        const geo = soft(scallop(lathe(TIER, sides, rows), lobes, spec.droop, seed + i), 0.02, 4, seed + i);
        parts.push(place(paint(geo, needles), { at: [shift, shift * 0.5, z], rot: [0, 0, spin], scale: [r, r, height] }));
    }
    if (spec.tufts && !bare) parts.push(tuft([0, 0, 1 - 0.5 * height], 0.35 * spec.radius, -1));
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

/** The crown of a `column` conifer. A swell is lighter and a hollow darker, so the lumps show in flat light. */
function column(spec: Conifer, needles: (p: V, n: V) => Colour, lod: Lod, seed: number): Geometry {
    const rows = [60, 24, 10][lod];
    const outline = Array.from({ length: rows + 1 }, (_, k): [number, number] => [spec.radius * spec.shape(k / rows), lerp(spec.base, 1, k / rows)]);
    const geo = soft(revolve([[0, spec.base], ...outline], DETAIL[lod].sides)), p = geo.position, swell: number[] = [];
    for (let i = 0; i < p.length; i += 3) {
        // Lumps taller than wide, as sprays that stand upright.
        swell.push(noise(v3(p[i], p[i + 1], 0.5 * p[i + 2]), 2.6 / spec.radius, seed));
        p[i] *= 1 + spec.jitter * swell[i / 3];
        p[i + 1] *= 1 + spec.jitter * swell[i / 3];
    }
    let k = 0;
    return paint(computeNormals(geo), (q, n) => needles(q, n).multiplyScalar(1 + 0.4 * swell[k++]));
}

/**
 * Bends a part made along z from 0 to 1 onto `path` from `from` to `to`. `across` gives where a point (x, y)
 * at z stands beside the path: level, and up.
 */
function bend(geo: Geometry, path: (s: number) => V, from: number, to: number, across: (x: number, y: number, z: number) => [number, number]): Geometry {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) {
        const s = lerp(from, to, p[i + 2]), along = path(s + 0.01).sub(path(s - 0.01)).normalize();
        const side = v3().crossVectors(UP, along);
        if (side.lengthSq() < 1e-6) side.set(1, 0, 0);
        const lift = along.cross(side.normalize()), [x, y] = across(p[i], p[i + 1], p[i + 2]);
        const at = path(s).addScaledVector(side, x).addScaledVector(lift, y);
        p.splice(i, 3, at.x, at.y, at.z);
    }
    return computeNormals(geo);
}

/**
 * Leans the normals of a thin part by `share` towards the way out of the tree and up, so that many thin parts
 * light as one round crown and not each as a slab.
 */
function rounded(geo: Geometry, share: number): Geometry {
    const n = v3(), p = v3();
    for (let k = 0; k < geo.normal.length / 3; k++) {
        n.from(geo.normal, k).lerp(p.from(geo.position, k).setZ(0).normalize().lerp(UP, 0.4).normalize(), share).normalize();
        geo.normal.splice(3 * k, 3, n.x, n.y, n.z);
    }
    return geo;
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

/** Crown lumps in the crown's own frame, radius 1: a core, an optional top and a ring around. */
function lumps(count: number, rand: () => number): { at: V; r: number }[] {
    const out = [{ at: v3(), r: 0.72 }];
    if (count >= 6) out.push({ at: v3(0, 0, 0.5), r: 0.5 });
    const ring = count - out.length;
    for (let k = 0; k < ring; k++) {
        const a = (k / ring) * 2 * Math.PI + rand() * 0.5;
        const z = (k % 2 ? 0.2 : -0.15) + (rand() - 0.5) * 0.15;
        out.push({ at: v3(0.55 * Math.cos(a), 0.55 * Math.sin(a), z), r: 0.48 + rand() * 0.08 });
    }
    return out;
}

/**
 * An open crown in its own frame, radius 1: a puff on top and smaller ones around it, with sky
 * between. More puffs are smaller; fewer huddle in a little, as a far tree has no limbs to hold them.
 */
function puffs(count: number, rand: () => number): { at: V; r: number }[] {
    const out = [{ at: v3(0.1, 0, 0.5), r: 0.5 }];
    const ring = count - 1;
    for (let k = 0; k < ring; k++) {
        const a = (k / ring) * 2 * Math.PI + rand() * 0.6;
        const d = clamp(ring / 5, 0.85, 1) * (0.62 + 0.25 * rand());
        out.push({ at: v3(d * Math.cos(a), d * Math.sin(a), -0.5 + 0.9 * rand()), r: Math.min(1, Math.sqrt(5 / ring)) * (0.32 + 0.1 * rand()) });
    }
    return out;
}

/** Dots on `share` of the vertices, fewer underneath. Far away an even tint, where dots would flicker. */
function fleck(p: V, n: V, share: number, lod: Lod, seed: number): number {
    const dot = hash(p, seed) < share ? 0.6 + 0.4 * smooth(-0.5, 0.5, n.z) : 0;
    return lerp(dot, 0.6 * share, lod / 2);
}

/**
 * A crown of soft lumps on a trunk. A bare crown is a limb into each lump, its branches and twigs
 * out to the lump's far side, with sky between them, so its outline stays at every distance.
 */
function broadleaf(spec: Broadleaf, season: Season, lod: Lod, seed: number): Built {
    const rand = random(seed);
    const [cz, rx, ry, rz] = spec.crown;
    const toTree = (p: V) => v3(p.x * rx, p.y * ry, cz + p.z * rz);
    const leaves = spec.leaves[season];
    const bare = leaves === 'bare';
    // A bare crown grows the limbs of the nearest level at every level.
    const { lumps: count, ico } = DETAIL[bare ? 0 : lod];
    const [deep, mid, tip] = ramp(bare ? spec.twigs : leaves);
    const flecks = spec.flecks?.[season];
    const foliage = (p: V, n: V) => {
        const c = deep
            .clone()
            .lerp(mid, 0.7 * smooth(-0.6, 0.3, n.z) + 0.3 * smooth(-0.8, 0.6, p.z))
            .lerp(tip, 0.7 * smooth(0.2, 0.9, n.z) * smooth(-0.2, 0.8, p.z));
        return flecks && !spec.bunches && !spec.rods ? c.lerp(hex(flecks[0]), fleck(p, n, flecks[1], lod, seed)) : c;
    };
    // A palm or a dragon tree grows its crown from the trunk's top.
    if (spec.fronds || spec.branching || spec.rods) {
        const crown = spec.fronds ? palm(spec, lod, [deep, mid, tip], rand)
            : spec.rods ? rods(spec, lod, bare, foliage, [mid, tip], flecks && hex(flecks[0]), rand, seed)
            : dragon(spec, lod, [deep, mid, tip], rand);
        // A far shrub's rods hide under its plumes, so it has one trunk as stout as all of them, up into the plumes.
        const bole = spec.rods && lod === 2 ? trunk({ ...spec, trunk: 1.4 * spec.trunk }, 0.35 * (cz + rz), lod, seed) : trunk(spec, spec.fork, lod, seed);
        return { geo: merge([bole, ...crown]), trunk: vertices([bole]), crown: lowest(crown) };
    }
    const crown = spec.open ? puffs(count + (spec.pads && !lod ? 3 : 0), rand) : lumps(count, rand);
    const parts: Geometry[] = [];
    // An open crown's trunk runs up into its top puff, which no limb reaches.
    const top = spec.open ? cz + 0.3 * rz : spec.fork + 0.3 * rz;
    // A far tree's stems hide under its crown, so it has one trunk as stout as all of them.
    const stems = lod < 2 ? (spec.stems ?? 1) : 1;
    const stout = { ...spec, trunk: spec.trunk * Math.sqrt((spec.stems ?? 1) / stems) };
    const leaders: { out: V; foot: V; lean: V }[] = [];
    for (let s = 0; spec.trunk && s < stems; s++) {
        const a = (s / stems) * 2 * Math.PI;
        // Stems lean apart from one stool, their feet a little apart.
        const out = stems > 1 ? v3(Math.cos(a), Math.sin(a), 0) : v3();
        const lean = out.clone().multiplyScalar(0.3).setZ(1).normalize();
        leaders.push({ out: out.clone(), foot: out.clone().multiplyScalar(1.2 * spec.trunk), lean });
        parts.push(place(trunk(stout, top, lod, seed + s, spec.crook ? [5, 3, 1][lod] : 1), { at: out.multiplyScalar(1.2 * spec.trunk), turn: turnFromTo(UP, lean) }));
    }
    // Where a limb towards `to` leaves the trunk at height z: the middle, or with `leaders` the stem on that side.
    const leave = (to: V, z: number) => {
        if (!spec.leaders || !leaders.length) return v3(0, 0, z);
        const { foot, lean } = leaders.reduce((best, stem) => (stem.out.dot(to) > best.out.dot(to) ? stem : best));
        return foot.clone().addScaledVector(lean, Math.min(z, 0.95 * top) / lean.z);
    };
    const [foot, bole] = spec.bark.map(hex);
    const bark = (p: V) => foot.clone().lerp(bole, smooth(0, spec.fork, p.z));
    if (bare) {
        const twigs = sheet();
        const [dead, share] = flecks ? [hex(flecks[0]), flecks[1]] : [mid, 0];
        const weep = spec.weep ?? 0;
        let kept = 0;
        for (const [n, { at, r }] of crown.entries()) {
            const out = at.clone().normalize(), centre = toTree(at), rise = rand();
            // No limb leaves the trunk above its top, so the top lump of an open crown grows from it.
            // Without a stagger the limbs leave over the lowest part of the way from the fork to their lumps.
            const fork = spec.stagger ? Math.max(spec.fork, centre.z - 0.6 * Math.hypot(centre.x, centre.y)) : lerp(spec.fork, Math.max(spec.fork, centre.z), 0.3 * rise);
            const from = leave(centre, Math.min(top, fork));
            bough(parts, twigs, from, centre, spec.trunk * 0.45, bark, DETAIL[lod].limbs, n, spec.trunk * 0.2);
            // The limb forks into branches that rise as they spread, and short twigs fan from their
            // outer parts to the lump's far side: a tree's tapering fork, not spikes from one point.
            for (let b = 0; b < BRANCHES; b++) {
                const way = v3(rand() - 0.5, rand() - 0.5, rand() - 0.2 - weep).multiplyScalar(1.6).add(out).normalize();
                const end = at.clone().addScaledVector(way, 0.6 * r);
                bough(parts, twigs, centre, toTree(end), spec.trunk * 0.2, bark, lod ? 0 : 4, BRANCHES * n + b, spec.trunk * 0.04);
                for (let k = 0; k < TWIGS / crown.length / BRANCHES; k++) {
                    const root = at.clone().lerp(end, 0.3 + 0.7 * rand());
                    const reach = root.clone().addScaledVector(v3(rand() - 0.5, rand() - 0.5, rand() - 0.3 - weep).add(way).normalize(), (0.2 + 0.25 * rand()) * r);
                    // A weeping tree's twigs hang below.
                    reach.z -= weep * rand() * r;
                    twig(twigs, toTree(root), toTree(reach), rand() < share ? [dead, dead] : [mid, tip], rand, lod, kept++);
                }
            }
        }
        parts.push(twigs.build());
    } else {
        crown.forEach(({ at, r }, k) => {
            // Each lump a little lighter or darker, so the crown does not read as one blob.
            const tone = 0.9 + 0.2 * rand();
            // A flat pad needs fewer corners than a round puff.
            const puff = soft(lump(r, spec.pads ? Math.min(ico, 2) : ico), 0.04, 4, seed + k);
            if (spec.weep) hang(puff, r, spec.weep);
            if (spec.pads) pad(puff, r);
            const geo = paint(place(puff, { at }), (p, n) => foliage(p, n).multiplyScalar(tone));
            parts.push(place(geo, { at: [0, 0, cz], scale: [rx, ry, rz] }));
        });
        // A bunch hangs from the side or the underside of a lump, where no other lump hides it.
        for (let k = 0, tries = 0; flecks && lod === 0 && k < (spec.bunches ?? 0) && tries < 1000; tries++) {
            const { at, r } = crown[Math.floor(rand() * crown.length)];
            const a = 2 * Math.PI * rand(), up = -0.75 + 0.9 * rand(), level = Math.sqrt(1 - up * up);
            const p = at.clone().addScaledVector(v3(level * Math.cos(a), level * Math.sin(a), up), 0.97 * r);
            if (crown.some(other => other.at !== at && other.at.distanceTo(p) < other.r)) continue;
            const size = 0.05 + 0.025 * rand(), fruit = hex(flecks[0]);
            const bunch = paint(soft(lathe(BUNCH, 5, 3)), (q) => mid.clone().lerp(fruit, 0.25 + 0.6 * smooth(0.1, -0.7, q.z)));
            parts.push(place(bunch, { at: toTree(p), rot: [0, 0, a], scale: [0.26 * size, 0.26 * size, size] }));
            k++;
        }
        // Limbs run from the fork into the outer lumps. An open crown keeps them a level further out,
        // so its puffs never float.
        if (spec.trunk && (lod === 0 || (spec.open && lod === 1))) {
            for (const { at } of crown.slice(1)) {
                const end = toTree(at.clone().multiplyScalar(0.8));
                const from = leave(end, spec.stagger ? Math.max(spec.fork, end.z - 0.6 * Math.hypot(end.x, end.y)) : spec.fork);
                if (spec.crook) {
                    // The limb leaves the trunk nearly level and turns up under its puff.
                    const knee = from.clone().lerp(end, 0.6).setZ(lerp(from.z, end.z, 0.25));
                    parts.push(tube([from, knee, end], spec.trunk * 0.45, spec.trunk * 0.3, lod ? 4 : 6, bark));
                } else parts.push(limb(from.toArray(), end.toArray(), spec.trunk * 0.45, bark, 0, lod ? 4 : 6));
            }
        }
    }
    const geo = merge(parts);
    if (spec.crook) crook(geo, spec.crook);
    if (spec.sweep) sweep(geo, spec.sweep);
    const trunks = spec.trunk ? stems : 0;
    return { geo, trunk: vertices(parts.slice(0, trunks)), crown: trunks ? lowest(parts.slice(trunks)) : 0 };
}

/**
 * Bare limb `k`, `radius` at its root and `tip` at its end: round with `sides` sides, or at 0 sides one
 * upright triangle among the twigs, which faces by k and draws no random number, so every level draws
 * the same twigs.
 */
function bough(parts: Geometry[], twigs: Sheet, from: V, to: V, radius: number, bark: (p: V) => Colour, sides: number, k: number, tip = radius): void {
    if (sides) parts.push(limb(from.toArray(), to.toArray(), radius, bark, 0, sides, tip));
    else upright(twigs, from, to, 2 * radius, [bark(from), bark(to)], k % 2 ? -1 : 1);
}

/** Bare stem `k` through `points`, as `bough`: a tube of `sides` sides, or at 0 sides a flat ribbon among the twigs. */
function stem(parts: Geometry[], twigs: Sheet, points: V[], radius: number, tip: number, bark: (p: V) => Colour, sides: number, k: number): void {
    if (sides) return void parts.push(tube(points, radius, tip, sides, bark));
    const last = points.length - 1, along = v3().subVectors(points[last], points[0]);
    const across = v3(0, 0, 1).addScaledVector(along, -along.z / along.lengthSq());
    if (across.lengthSq() < 1e-6) across.set(1, 0, 0);
    across.setLength(k % 2 ? -1 : 1);
    const edge = (i: number, sign: number) => points[i].clone().addScaledVector(across, sign * lerp(radius, i === last ? 0 : tip, i / last));
    for (let i = 0; i < last; i++) {
        const [a, b] = [bark(points[i]), bark(points[i + 1])];
        twigs.add([edge(i, -1), edge(i, 1), edge(i + 1, 1)], [a, a, b]);
        if (i + 1 < last) twigs.add([edge(i, -1), edge(i + 1, 1), edge(i + 1, -1)], [a, b, b]);
    }
}

// A hanging bunch, 1 long below the point that it hangs from, and a catkin or a plume of leaves along z from 0 to 1.
const BUNCH: [number, number][] = [[0, -1], [0.9, -0.62], [0.75, -0.2], [0, 0.12]];
const PLUME: [number, number][] = [[0, 0], [0.7, 0.08], [1, 0.3], [0.92, 0.6], [0.72, 0.88], [0, 1]];

/**
 * The crown of a `rods` shrub: the rods fan from the stool, the outer ones lower and leaning further, each with
 * two side shoots. Leafy, a plume wraps each rod's upper part and each shoot; bare, twigs stand along them.
 */
function rods(spec: Broadleaf, lod: Lod, bare: boolean, foliage: (p: V, n: V) => Colour, [, tip]: Colour[], catkin: Colour | undefined, rand: () => number, seed: number): Geometry[] {
    const [cz, rx, ry, rz] = spec.crown, n = spec.rods!, sides = [4, 3, 0][lod];
    const foot = hex(spec.bark[0]), wood = hex(spec.twigs[1]), young = hex(spec.twigs[2]);
    const bark = (p: V) => foot.clone().lerp(wood, smooth(0, 0.2, p.z)).lerp(young, smooth(0.3, 0.9, p.z));
    const leaf = (tone: number) => (p: V, normal: V) => foliage(v3(p.x / rx, p.y / ry, (p.z - cz) / rz), normal).multiplyScalar(tone);
    const parts: Geometry[] = [], twigs = sheet();
    let kept = 0;
    // A plume around `path`, `width` across, in `tufts` tufts that stand out to one side after the other: a leafy
    // wand, not a blade. A far one has fewer corners.
    const plume = (path: (s: number) => V, from: number, width: number, k: number, tufts: number) => {
        const geo = soft(lathe(PLUME, [6, 4, 3][lod], [2 * tufts, 4, 2][lod]), 0.1, 4, seed + k);
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
        const height = (cz + rz) * lerp(1, 0.7, ring ** 1.5) * (0.92 + 0.16 * rand());
        const reach = ring * (0.85 + 0.3 * rand()), start = 1.2 * spec.trunk * ring;
        // A rod splays at the stool, then stands; its tip nods out.
        const path = (s: number) => {
            const k = 0.6 * Math.min(1, Math.max(0, s)) ** 0.7 + 0.4 * s ** 3;
            return v3(Math.cos(a) * lerp(start, reach * rx, k), Math.sin(a) * lerp(start, reach * ry, k), lerp(0.5 * spec.fork, height, s));
        };
        stem(parts, twigs, [0, 0.2, 0.5, 1].map(path), 0.4 * spec.trunk, 0.1 * spec.trunk, bark, sides, i);
        const width = 0.034 * (0.8 + 0.4 * rand());
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
        // A catkin stands on the plume, up and out.
        for (let c = 0; catkin && !bare && lod === 0 && c < 3; c++) {
            const s = 0.4 + 0.5 * rand(), turn = 2 * Math.PI * rand(), out = v3(Math.cos(turn), Math.sin(turn), 0);
            const geo = paint(soft(lathe([[0, 0], [1, 0.4], [0, 1]], 4, 2)), catkin);
            parts.push(place(geo, { at: path(s).addScaledVector(out, width), turn: turnFromTo(UP, out.setZ(1.2).normalize()), scale: [0.011, 0.011, 0.065] }));
        }
    }
    return [...parts, twigs.build()];
}

/** Twig `k` of a bare crown at a level: every `thin`-th twig stands, `thin` times as wide (DETAIL, TWIGS). */
function twig(twigs: Sheet, root: V, tip: V, colours: Colour[], rand: () => number, lod: Lod, k: number): void {
    const { thin } = DETAIL[lod], face = rand() < 0.5 ? 1 : -1;
    if (k % thin === 0) upright(twigs, root, tip, thin * TWIG_WIDTH, colours, face);
}

/**
 * One upright triangle from `width` across at its root to a point at its tip, facing `face` (±1).
 * Upright, it shows from the side and its normal stays level; twigs face either way, so half of them
 * catch the sun wherever they are in the crown.
 */
function upright(twigs: Sheet, root: V, tip: V, width: number, [base, end]: Colour[], face: number): void {
    const along = v3().subVectors(tip, root);
    const across = v3(0, 0, 1).addScaledVector(along, -along.z / along.lengthSq());
    if (across.lengthSq() < 1e-6) across.set(1, 0, 0);
    across.setLength(0.5 * face * width);
    twigs.add([root.clone().sub(across), root.clone().add(across), tip], [base, base, end]);
}

/**
 * Fronds arch out of a knob of leaf bases, the young ones upright and the old ones hanging. Near
 * the eye each leaflet is one card, so a frond reads as a comb: 48 fronds of 11 pairs. Further off
 * a frond is one strip over the comb's outline, which keeps the crown's mass.
 */
function palm(spec: Broadleaf, lod: Lod, [deep, mid, tip]: Colour[], rand: () => number): Geometry[] {
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
    const knob = paint(place(soft(lump((lod ? 1.3 : 1.7) * spec.trunk, lod ? 0 : 1), 0.1 * spec.trunk, 30), { at: base }), hex('#6a5a3c').lerp(deep, lod ? 0.6 : 0));
    return [cards.build(), knob];
}

/**
 * Limbs fork `branching` ways at each level, rising and spreading so the rosettes of sword
 * leaves at their ends make an umbrella. A tree within 200 m keeps two levels; a far one keeps
 * the first, each limb ending in a ragged bowl where its rosettes would be.
 */
function dragon(spec: Broadleaf, lod: Lod, [deep, mid, tip]: Colour[], rand: () => number): Geometry[] {
    const [cz, rx, , rz] = spec.crown;
    const levels = spec.branching!.slice(0, [3, 2, 1][lod]);
    const [foot, bole] = spec.bark.map(hex);
    const bark = (p: V) => foot.clone().lerp(bole, smooth(0, spec.fork, p.z));
    const size = 0.2;
    const blades = lod ? 20 : 32;
    const fork = v3(0, 0, spec.fork);
    // A collar where the limbs leave the trunk, so the trunk swells into them.
    const parts = lod < 2 ? [paint(place(soft(lump(1.1 * spec.trunk, lod ? 0 : 1), 0.08 * spec.trunk, 30), { at: fork }), bark)] : [];
    const cards = sheet();
    const grow = (from: V, a0: number, a1: number, level: number, r: number) => {
        const m = levels[level];
        for (let c = 0; c < m; c++) {
            const a = a0 + ((c + 0.5 + 0.4 * (rand() - 0.5)) / m) * (a1 - a0);
            // A far tree's one level reaches as far as a near tree's first.
            const f = (level + 1) / Math.max(2, levels.length);
            const d = rx * f ** 0.8 * (0.75 + 0.35 * rand());
            const z = spec.fork + (cz + rz * Math.sqrt(Math.max(0, 1 - (d / rx) ** 2)) - size - spec.fork) * f ** 0.45;
            const to = v3(d * Math.cos(a), d * Math.sin(a), z);
            parts.push(limb(from.toArray(), to.toArray(), r, bark, 0, [6, 4, 3][lod]));
            if (level + 1 < levels.length) grow(to, a0 + (c * (a1 - a0)) / m, a0 + ((c + 1) * (a1 - a0)) / m, level + 1, 0.75 * r);
            else if (lod === 2) bowl(cards, to, a, 0.7 * rx, size, [deep, mid, tip]);
            else {
                rosette(cards, to, size, blades, [deep, mid, tip], rand);
                // A heart fills the rosette between its blades.
                parts.push(paint(place(soft(lump(0.38 * size, 0)), { at: to.clone().setZ(to.z + 0.15 * size) }), deep.clone().lerp(mid, 0.5)));
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

/** Stiff sword leaves up and out from one point, only the outer ring drooping. */
function rosette(cards: Sheet, at: V, size: number, count: number, [deep, mid, tip]: Colour[], rand: () => number): void {
    const light = domed(at, v3(1, 1, 1), 0.3);
    for (let j = 0; j < count; j++) {
        const a = j * GOLDEN + 0.5 * rand();
        const t = (j + 0.5) / count;
        const polar = 1.3 * Math.sqrt(t) + 0.45 * smooth(0.7, 1, t);
        const reach = size * (0.8 + 0.35 * rand());
        const end = at.clone().add(v3(Math.sin(polar) * Math.cos(a), Math.sin(polar) * Math.sin(a), Math.cos(polar)).multiplyScalar(reach));
        const side = v3(-Math.sin(a), Math.cos(a), 0).multiplyScalar(0.14 * reach);
        cards.add([at.clone().sub(side), at.clone().add(side), end], [deep, deep, mid.clone().lerp(tip, smooth(0.5, 0, polar) + 0.3 * rand())], light);
    }
}

/** Bends a tree from side to side on its way up, by `amount` tree heights; its foot stays. */
function crook(geo: Geometry, amount: number): void {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) {
        const k = amount * smooth(0, 0.25, p[i + 2]);
        p[i] += k * Math.sin(9 * p[i + 2]);
        p[i + 1] += k * Math.sin(6.5 * p[i + 2] + 2);
    }
    computeNormals(geo);
}

/** Squashes a lump of radius r into a pad with a flat top and round sides. */
function pad(geo: Geometry, r: number): void {
    const p = geo.position;
    for (let i = 2; i < p.length; i += 3) p[i] -= 0.4 * r * smooth(0.2 * r, r, p[i]);
    computeNormals(geo);
}

/** Draws the underside of a lump of radius r down into a hanging point. */
function hang(geo: Geometry, r: number, amount: number): void {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) {
        const k = smooth(0.2, -1, p[i + 2] / r);
        const narrow = 1 - 0.5 * amount * k;
        p.splice(i, 3, p[i] * narrow, p[i + 1] * narrow, p[i + 2] - amount * r * k * k);
    }
    computeNormals(geo);
}

/** Shears a tree downwind, towards +x, more the higher it reaches. */
function sweep(geo: Geometry, amount: number): void {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) p[i] += amount * p[i + 2] ** 2;
    computeNormals(geo);
}
