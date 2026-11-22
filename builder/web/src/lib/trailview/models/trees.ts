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
    /** Buttresses around the foot that run out up the trunk, as on a cedar: how many. A near trunk has two sides a buttress. */
    flutes?: number;
    /** The flutes turn this far round the trunk on their way up, in turns, and run to its top, as on an old olive. */
    twist?: number;
}

interface Base extends Bark {
    name: string;
    latin: string;
    /** Placed at the heading it is built in, as a tree the wind bends must be; others turn at random. */
    keepsHeading?: true;
    /** Berries in clusters of three on the crown of a near tree, as on a juniper: their colour and how many clusters. */
    berries?: [string, number];
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
    /** This many tiers or whorls in place of the level's own, a tier with fewer rows: thin plates of level branches, as on a fir. */
    whorls?: number;
    /** The top tier is flat, so the crown ends in a nest in place of a point, as an old fir's. */
    nest?: true;
    /** Pale needle undersides, as on silver fir. */
    under?: string;
    /** How far the leader nods over towards +x, in tree heights. */
    nod?: number;
    /**
     * Each tier a whorl of `lobes` branches that end in tufts of needles, as on a pine: a tuft's radius in tier radii.
     * Under a half, sky shows between the tufts; near 1 they close into a dense crown.
     */
    tufts?: number;
    /** A whorl's branches rise this far, in their length, and their tufts stand along them, as the brushes of a mountain pine. */
    lift?: number;
    /** Needle tufts sprouting from the bare trunk: how many a near tree has. */
    sprouts?: number;
    /** One smooth column over the crown's outline in soft upright lumps, `jitter` deep, as a cypress: no tiers. */
    column?: true;
    /** A column has this many lower tops that lean out of its sides, as the stems of a juniper. */
    spires?: number;
    /**
     * A column's skin is in round knobs, each four corners wide and tall, in upright rows that break joint, as the needle
     * clumps of a juniper, in place of upright lumps; `jitter` is how far they stand out, in column radii.
     */
    knobs?: true;
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
    /**
     * The crown's outline: its radius in crown half-widths from its base (t = 0) to its top (t = 1). `clumps` lumps stand
     * on it near the eye, 12 by default, more where it is wide, each on its own limb: the lowest on main limbs from the
     * fork, or with `stagger` from a trunk that runs to the top, the others on limbs that fork from the main ones. An
     * `open` crown has sky between its lumps; another has a core that closes it.
     */
    shape?: (t: number) => number;
    clumps?: number;
    /** A shaped crown's limbs bow: above 0 they rise from the trunk and arch out, below 0 they leave it level and turn up. */
    bow?: number;
    /**
     * The flecks of a leafy crown are heads that sit on it, as flower heads, plumes or berry clusters, not dots: how many
     * a near tree has where the season's share is 1, a head's width in tree heights and its height in widths, and the
     * points of a head that is a star, as a burst of catkins.
     */
    heads?: { count: number; width: number; height: number; points?: number };
    /** Puffs are flat-topped pads, as on a Scots pine; a near tree has three more. */
    pads?: true;
    /** Each lump swells into this many round tufts, lighter on the swells, as the needle brushes of a pine. */
    billows?: number;
    /** Puffs this many times as wide: an open crown of few, heavy puffs. In a shaped open crown, a lump's width as a share of one that closes the crown, 0.6 by default. */
    dense?: number;
    /** An open crown's puffs stand in this many level whorls under a dome, the widest lowest, as on a black pine: near the eye 3 or 4 a whorl. */
    layers?: number;
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
     * shrub willow: how many it has. Its flecks are catkins that stand on the plumes, three a rod, or with `heads` the
     * season's share of that many flowers, over a little of their colour on the plumes. `dense` is a plume's width against
     * a willow's: under 0.7 a slim green switch, as on a broom.
     */
    rods?: number;
    /**
     * The flecks of a leafy crown are bunches that hang from it, as a hop hornbeam's fruit, not dots: how many a near tree
     * has. A bare shaped crown has as many catkins that hang from its twigs.
     */
    bunches?: number;
    /** A palm: this many fronds near the eye arch out of the trunk's top, as long as the crown is wide. */
    fronds?: number;
    /** A dragon tree: limbs fork this many ways at each level and end in rosettes of sword leaves. */
    branching?: number[];
    leaves: Record<Season, Ramp | 'bare'>;
    /** The colour of a bare crown's twigs. */
    twigs: Ramp;
    /**
     * Blossom, berries, catkins or dead leaves by season: colour and share. A leafy crown shows them as its `heads`,
     * its `bunches` or a `rods` shrub's catkins; a bare crown as whole twigs, as catkins or dead leaves that hang on.
     */
    flecks?: Partial<Record<Season, [string, number]>>;
}

/** A species file in trees/. */
export type Tree = Conifer | Broadleaf;

/** A tree 1 tall: its first `trunk` vertices are its trunk, and `crown` is the lowest point of the rest in tree heights. */
export interface Grown extends Geometry { trunk: number; crown: number }

export function grow(spec: Tree, season: Season, lod: Lod, seed: number): Grown {
    const { geo, crown, trunk } = spec.form === 'conifer' ? conifer(spec, season, lod, seed) : broadleaf(spec, season, lod, seed);
    // No crown reaches under the ground: low boughs and lumps that would lie on it. Their normals stay, so they light as before.
    const p = geo.position;
    for (let k = 3 * trunk + 2; k < p.length; k += 3) p[k] = Math.max(0, p[k]);
    const top = Math.max(...p.filter((_, i) => i % 3 === 2));
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
function trunk({ trunk: r, flare, neck = 0.6, bark, fade = [0, 0.5], marks, scars, flutes, twist }: Bark, top: number, lod: Lod, seed: number, spans = 1): Geometry {
    if (scars && lod === 0) return scarred(r, flare, neck, bark, scars, top);
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
    if (flutes) {
        const p = geo.position;
        for (let i = 0; i < p.length; i += 3) {
            // A side stands out and the next stands in, less the higher up.
            const k = 1 + 0.3 * Math.cos(flutes * (Math.atan2(p[i + 1], p[i]) + Math.PI / 2 + (2 * Math.PI * (twist ?? 0) * p[i + 2]) / top)) * smooth(twist ? 1.5 * top : 0.5 * top, 0, p[i + 2]);
            p[i] *= k;
            p[i + 1] *= k;
        }
        computeNormals(geo);
    }
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
 * 4 × n triangles a ring. The trunk swells to `neck` radii under the crown.
 */
function scarred(r: number, flare: number, neck: number, bark: [string, string], n: number, top: number): Geometry {
    const [foot, bole] = bark.map(hex);
    // Rings half a diamond apart; a diamond is half as tall as it is wide.
    const h = (0.25 * 2 * Math.PI * r) / n;
    const corner = (i: number, k: number, raised = false) => {
        const z = i * h;
        // Scars run in a slow spiral, as the leaves grew.
        const a = (k / 2 + 0.03 * i) * ((2 * Math.PI) / n);
        const radius = r * (1 + (flare - 1) * (1 - smooth(0, 4 * r, z))) * lerp(1, neck, smooth(0.4 * top, top, z)) * (raised ? 1.15 : 1);
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
                // A knob stands out along the skin's normal, so the top is in knobs too.
                // A knob swells from its edges to its middle and stands out along the skin's normal, some further. The knobs
                // of one upright row start a quarter, a half or three quarters of a knob higher than those of the next.
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

/** A berry's radius, in tree heights. */
const BEAD = 0.009;

/**
 * Berry clusters on the skin of a crown's `parts`, three beads each, at corners on its side that `hidden` does not rule
 * out, as corners inside another part.
 */
function berries(parts: Geometry[], [colour, count]: [string, number], rand: () => number, hidden: (p: V, part: number) => boolean = () => false): Geometry[] {
    const out: Geometry[] = [], shade = hex(colour), p = v3(), n = v3();
    for (let tries = 0; out.length < 3 * count && tries < 1000; tries++) {
        const part = Math.floor(rand() * parts.length), k = Math.floor((rand() * parts[part].position.length) / 3);
        p.from(parts[part].position, k);
        if (Math.abs(n.from(parts[part].normal, k).z) > 0.6 || hidden(p, part)) continue;
        for (let b = 0; b < 3; b++) {
            const a = 2 * Math.PI * (b / 3 + rand()), at = p.clone().addScaledVector(n, 0.5 * BEAD).add(v3(0.9 * BEAD * Math.cos(a), 0.9 * BEAD * Math.sin(a), BEAD * (rand() - 0.5)));
            out.push(place(paint(soft(lump(BEAD, 0)), (_, m) => shade.clone().multiplyScalar(0.8 + 0.4 * smooth(-0.5, 1, m.z))), { at }));
        }
    }
    return out;
}

// A flower head, radius 1 and 1 tall, over a collar that stands in the crown.
const HEAD: [number, number][] = [[0.9, -0.3], [1, 0.1], [0.8, 0.6], [0, 1]];

/**
 * Heads on the skin of a crown's `parts`, as `berries`: each faces out of the crown and up, in the fleck's colour over
 * a collar of the leaves' colour `leaf`.
 */
function heads(parts: Geometry[], { count, width, height, points }: NonNullable<Broadleaf['heads']>, [colour, share]: [string, number], leaf: Colour, rand: () => number, hidden: (p: V, part: number) => boolean): Geometry[] {
    const out: Geometry[] = [], shade = hex(colour), p = v3(), n = v3();
    for (let tries = 0; out.length < Math.round(count * share) && tries < 2000; tries++) {
        const part = Math.floor(rand() * parts.length), k = Math.floor((rand() * parts[part].position.length) / 3);
        p.from(parts[part].position, k);
        if (n.from(parts[part].normal, k).z < -0.3 || hidden(p, part)) continue;
        // A star is flat: a point in the middle and a rim.
        const geo = points ? soft(revolve([[1, 0], [0, 0.4]], 2 * points)) : soft(lathe(HEAD, 6, 2), 0.1, 5, tries), q = geo.position;
        // A star's points stand out and hang a little.
        for (let i = 0; points && i < q.length; i += 3) {
            const far = 0.5 + 0.5 * Math.cos(points * Math.atan2(q[i + 1], q[i])), k = lerp(0.3, 1.4, far);
            q.splice(i, 3, q[i] * k, q[i + 1] * k, q[i + 2] - 0.4 * far * Math.hypot(q[i], q[i + 1]));
        }
        const tone = 0.85 + 0.3 * rand(), size = 0.5 * width * (0.8 + 0.4 * rand());
        const head = paint(computeNormals(geo), (at, m) => leaf.clone().lerp(shade.clone().multiplyScalar(tone * (0.85 + 0.2 * m.z)), smooth(-0.15, 0.1, at.z)));
        out.push(place(head, { at: p.clone(), turn: turnFromTo(UP, n.lerp(UP, 0.5).normalize()), scale: [size, size, 2 * size * height] }));
    }
    return out;
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

/** A part without the triangles whose corners `hidden` all picks, and without the corners that no triangle is left on. */
function shown(geo: Geometry, hidden: (p: V) => boolean): Geometry {
    const p = v3(), gone = Array.from({ length: geo.position.length / 3 }, (_, k) => hidden(p.from(geo.position, k)));
    const at = new Map<number, number>(), out: Geometry = { position: [], normal: [], color: [], index: [] };
    for (let t = 0; t < geo.index.length; t += 3) {
        const corners = geo.index.slice(t, t + 3);
        if (corners.every(k => gone[k])) continue;
        for (const k of corners) {
            if (!at.has(k)) {
                at.set(k, at.size);
                for (const name of ['position', 'normal', 'color'] as const) out[name].push(...geo[name].slice(3 * k, 3 * k + 3));
            }
            out.index.push(at.get(k)!);
        }
    }
    return out;
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
 * A shaped crown in its own frame, from -1 to 1 high: a lump on top, first, and the others around the outline from low
 * to high, a part of a turn on each, at heights that share the outline's area evenly. `size` times as large as lumps
 * that close the crown.
 */
function clumps(shape: (t: number) => number, count: number, rand: () => number, size: number): { at: V; r: number }[] {
    const ring = count - 1, r0 = (size * 1.75) / Math.sqrt(count), STEPS = 24;
    const wide = Array.from({ length: STEPS }, (_, i) => shape((i + 0.5) / STEPS)), total = wide.reduce((sum, w) => sum + w, 0), widest = Math.max(...wide);
    const top = clamp(1.3 * shape(0.85), 0.6 * r0, 1.1 * r0), out = [{ at: v3(0, 0, 1 - top), r: top }];
    for (let k = 0, i = 0, below = 0; k < ring; k++) {
        const want = ((k + 0.5) / ring) * total;
        while (below + wide[i] < want) below += wide[i++];
        const t = lerp(0.08, 0.82, (i + (want - below) / wide[i]) / STEPS), edge = shape(t);
        const r = Math.min(r0 * lerp(0.7, 1.1, edge / widest) * (0.85 + 0.3 * rand()), Math.max(edge, 0.5 * r0)), a = k * GOLDEN + 0.5 * rand();
        // A lump stands a little out of the outline, a large one no further than a small one.
        const d = Math.max(0, edge - r + 0.12);
        out.push({ at: v3(d * Math.cos(a), d * Math.sin(a), Math.max(2 * t - 1, 0.8 * r - 1)), r });
    }
    return out;
}

/**
 * An open crown in its own frame, radius 1: a puff on top and smaller ones around it, with sky
 * between. More puffs are smaller; fewer huddle in a little, as a far tree has no limbs to hold them.
 */
function puffs(count: number, rand: () => number, dense = 1, layers = 0): { at: V; r: number }[] {
    const out = [{ at: v3(0.1, 0, 0.5), r: 0.5 }];
    const ring = count - 1;
    for (let k = 0; k < ring; k++) {
        if (layers) {
            // Whorl after whorl takes a puff, each a part of a turn on from the one below.
            const j = k % layers, n = Math.ceil((ring - j) / layers), z = lerp(-0.75, 0.1, j / Math.max(1, layers - 1));
            const a = ((Math.floor(k / layers) + 0.5 * j + 0.3 * rand()) / n) * 2 * Math.PI, d = clamp(ring / 8, 0.5, 1) * 0.8 * Math.sqrt(1 - ((z + 0.75) / 1.9) ** 2);
            out.push({ at: v3(d * Math.cos(a), d * Math.sin(a), z + 0.1 * (rand() - 0.5)), r: dense * Math.min(1, Math.sqrt(8 / ring)) * (0.34 + 0.06 * rand()) });
            continue;
        }
        const a = (k / ring) * 2 * Math.PI + rand() * 0.6;
        const d = clamp(ring / 5, 0.85, 1) * (0.62 + 0.25 * rand());
        out.push({ at: v3(d * Math.cos(a), d * Math.sin(a), -0.5 + 0.9 * rand()), r: dense * Math.min(1, Math.sqrt(5 / ring)) * (0.32 + 0.1 * rand()) });
    }
    return out;
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
    const { lumps: count, ico: corners } = DETAIL[bare ? 0 : lod];
    // The lumps of a shaped crown: a leafy one has fewer, larger ones a level. Its far tier is simplified from its far
    // level, whose lumps are round enough to stay lumps.
    const clumped = spec.shape && Math.round((spec.clumps ?? 12) * (bare ? 1 : [1, 0.75, 0.55][lod]));
    const ico = clumped ? Math.max(1, corners) : corners;
    const [deep, mid, tip] = ramp(bare ? spec.twigs : leaves);
    const flecks = spec.flecks?.[season];
    const foliage = (p: V, n: V) => {
        const c = deep
            .clone()
            .lerp(mid, 0.7 * smooth(-0.6, 0.3, n.z) + 0.3 * smooth(-0.8, 0.6, p.z))
            .lerp(tip, 0.7 * smooth(0.2, 0.9, n.z) * smooth(-0.2, 0.8, p.z));
        // Under its heads a crown takes a little of their colour on its upper sides, which a far tier keeps.
        return flecks && spec.heads ? c.lerp(hex(flecks[0]), 0.5 * flecks[1] * smooth(0, 0.8, n.z)) : c;
    };
    // A palm or a dragon tree grows its crown from the trunk's top.
    if (spec.fronds || spec.branching || spec.rods) {
        const crown = spec.fronds ? palm(spec, lod, [deep, mid, tip], rand)
            : spec.rods ? rods(spec, lod, bare, foliage, [mid, tip], flecks ? [hex(flecks[0]), flecks[1]] : [], rand, seed)
            : dragon(spec, lod, [deep, mid, tip], rand);
        // A far shrub's rods hide under its plumes, so it has one trunk as stout as all of them, up into the plumes.
        const bole = spec.rods && lod === 2 ? trunk({ ...spec, trunk: 1.4 * spec.trunk }, 0.35 * (cz + rz), lod, seed) : trunk(spec, spec.fork, lod, seed);
        return { geo: merge([bole, ...crown]), trunk: vertices([bole]), crown: lowest(crown) };
    }
    // A far crooked tree has a puff more, as it keeps its limbs.
    const whorled = spec.layers && [Math.round(3.6 * spec.layers), Math.round(2.4 * spec.layers), 4][lod] + 1;
    const crown = clumped ? clumps(spec.shape!, clumped, rand, spec.open ? (spec.dense ?? 0.6) : 1) : spec.open ? puffs((whorled || count + (spec.pads && !lod ? 3 : 0)) + (spec.crook && lod === 2 ? 1 : 0), rand, spec.dense, spec.layers) : lumps(count, rand);
    const parts: Geometry[] = [];
    // An open crown's trunk runs up into its top puff, which no limb reaches.
    const high = spec.open ? cz + (spec.layers ? 0.5 : 0.3) * rz : spec.fork + 0.3 * rz;
    // A shaped crown's trunk ends where its limbs leave: under the top lump with `stagger`, in the crown's middle where
    // it has stems, else at the fork. A far open one runs to the middle, as its limbs are too thin to keep.
    const top = !clumped ? high : spec.stagger ? (spec.stems ? cz : toTree(crown[0].at).z - crown[0].r * rz) : spec.open && lod === 2 && !bare ? cz : spec.fork + 0.1 * rz;
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
        parts.push(place(trunk(stout, top, lod, seed + s, spec.crook ? [5, 3, clumped ? 1 : 3][lod] : 1), { at: out.multiplyScalar(1.2 * spec.trunk), turn: turnFromTo(UP, lean) }));
    }
    // Where a limb towards `to` leaves the trunk at height z: the middle, or with `leaders` the stem on that side.
    const leave = (to: V, z: number) => {
        if (!spec.leaders || !leaders.length) return v3(0, 0, z);
        const { foot, lean } = leaders.reduce((best, stem) => (stem.out.dot(to) > best.out.dot(to) ? stem : best));
        return foot.clone().addScaledVector(lean, Math.min(z, 0.95 * top) / lean.z);
    };
    const [foot, bole] = spec.bark.map(hex);
    const bark = (p: V) => foot.clone().lerp(bole, smooth(0, spec.fork, p.z));
    // The limbs of a shaped crown, by lump: a main limb leaves the trunk, another forks from the main limb on its side.
    // A bare limb runs to its lump's middle. The top lump's comes last, as it forks from a main limb.
    const MAINS = 5;
    const main = (n: number) => !!spec.stagger || (n > 0 && n <= MAINS);
    const limbs: ((s: number) => V)[] = [];
    for (const n of clumped ? [...crown.keys()].slice(1).concat(0) : []) {
        const end = toTree(crown[n].at.clone().multiplyScalar(bare ? 1 : 0.8)), level = Math.hypot(end.x, end.y);
        const from = spec.stagger ? leave(end, n ? clamp(end.z - 0.6 * level, spec.fork, top) : top)
            : main(n) ? leave(end, spec.fork)
            : limbs.slice(1, MAINS + 1).reduce((best, limb) => (limb(1).setZ(0).normalize().dot(end) > best(1).setZ(0).normalize().dot(end) ? limb : best))(0.45);
        const length = from.distanceTo(end), side = level > 1e-6 ? v3(-end.y, end.x, 0).normalize() : v3(1, 0, 0);
        // The limb's bend: up or down by `bow`, and a little to one side.
        const knee = from.clone().lerp(end, 0.5).addScaledVector(UP, 0.5 * (spec.bow ?? 0) * length).addScaledVector(side, 0.12 * length * (hash(end, seed) - 0.5));
        limbs[n] = (s: number) => from.clone().multiplyScalar((1 - s) ** 2).addScaledVector(knee, 2 * s * (1 - s)).addScaledVector(end, s * s);
    }
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
            if (clumped) stem(parts, twigs, (lod < 2 ? [0, 0.33, 0.67, 1] : [0, 0.5, 1]).map(limbs[n]), (main(n) ? 0.45 : 0.3) * spec.trunk, spec.trunk * 0.15, bark, DETAIL[lod].limbs, n);
            else bough(parts, twigs, from, centre, spec.trunk * 0.45, bark, DETAIL[lod].limbs, n, spec.trunk * 0.2);
            // The limb forks into branches that rise as they spread, and short twigs fan from their
            // outer parts to the lump's far side: a tree's tapering fork, not spikes from one point.
            for (let b = 0; b < BRANCHES; b++) {
                const way = v3(rand() - 0.5, rand() - 0.5, rand() - 0.2 - weep).multiplyScalar(1.6).add(out).normalize();
                const end = at.clone().addScaledVector(way, (clumped ? 0.85 : 0.6) * r);
                bough(parts, twigs, centre, toTree(end), spec.trunk * 0.2, bark, lod ? 0 : 4, BRANCHES * n + b, spec.trunk * 0.04);
                // A shaped crown's twigs fork in threes, at the branch's end and along it.
                for (let k = 0, fans = Math.round(TWIGS / crown.length / BRANCHES / 3); clumped && k < fans; k++) {
                    const root = at.clone().lerp(end, 1 - (0.7 * (k + 0.5 * rand())) / fans);
                    const along = v3(rand() - 0.5, rand() - 0.5, rand() - 0.3 - weep).add(way).normalize(), across = v3(rand() - 0.5, rand() - 0.5, rand() - 0.5).cross(along).normalize();
                    const colours = rand() < share ? [dead, dead] : [mid, tip], length = (0.4 + 0.4 * rand()) * r;
                    for (const fork of [0, -1, 1]) {
                        const reach = root.clone().addScaledVector(along.clone().addScaledVector(across, 0.55 * fork).normalize(), length * (fork ? 0.8 : 1));
                        reach.z -= weep * rand() * r;
                        twig(twigs, toTree(root), toTree(reach), colours, rand, lod, kept++);
                        // A catkin hangs from a twig, as many on the crown as it has bunches.
                        if (flecks && !lod && Math.floor((kept * (spec.bunches ?? 0)) / TWIGS) > Math.floor(((kept - 1) * (spec.bunches ?? 0)) / TWIGS)) {
                            const size = 0.035 + 0.03 * hash(reach, seed);
                            parts.push(place(paint(soft(lathe(BUNCH, 4, 2)), (q) => dead.clone().multiplyScalar(0.75 + 0.35 * smooth(-1, 0, q.z))), { at: toTree(root.clone().lerp(reach, 0.6)), scale: [0.16 * size, 0.16 * size, size] }));
                        }
                    }
                }
                if (clumped) continue;
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
        // A closed shaped crown has a core inside its lumps.
        if (clumped && !spec.open) {
            const core = soft(lathe([[0, -0.9], ...Array.from({ length: 5 }, (_, j): [number, number] => [0.7 * spec.shape!(0.1 + 0.2 * j), -0.8 + 0.4 * j]), [0, 0.9]], [8, 6, 4][lod], [6, 4, 3][lod]));
            parts.push(place(paint(core, (p, n) => foliage(p, n).multiplyScalar(0.85)), { at: [0, 0, cz], scale: [rx, ry, rz] }));
        }
        // A shaped crown's lumps are less tall in a crown taller than wide, and taller in a wide one, so they are nearly round.
        const tall = clumped ? clamp((rx / rz) ** 0.6, 0.6, 1.3) : 1;
        crown.forEach(({ at, r }, k) => {
            // Each lump a little lighter or darker, so the crown does not read as one blob.
            const tone = 0.9 + 0.2 * rand();
            // A flat pad needs fewer corners than a round puff, and so does a small puff that swells.
            const puff = soft(lump(r, spec.pads || ((spec.billows || clumped) && r < 0.44) ? Math.min(ico, 2) : ico), 0.04, 4, seed + k);
            // A far lump has too few corners to swell.
            const swell = spec.billows && ico > 1 ? billow(puff, r, spec.billows, seed + k) : undefined;
            if (spec.weep) hang(puff, r, spec.weep);
            if (spec.pads) pad(puff, r);
            let v = 0;
            // A whorl's puff is no taller in a crown taller than wide; a far one has too few corners for a pad, and is flat.
            const whole = paint(place(puff, { at, scale: [1, 1, spec.layers ? Math.min(ico < 2 ? 0.6 : 1, rx / rz) : tall] }), (p, n) => foliage(p, n).multiplyScalar(tone * (swell ? 0.8 + 0.4 * swell[v++] : 1)));
            // A shaped crown's lump keeps the skin that shows: not what stands in another lump or in the core. A lump is
            // less deep where it swells between tufts or hangs to a point.
            const sunk = (swell ? 0.62 : 0.88) * (1 - 0.5 * (spec.weep ?? 0));
            const within = (other: V, p: V) => Math.hypot(p.x - other.x, p.y - other.y, (p.z - other.z) / tall);
            const geo = clumped ? shown(whole, p => crown.some((other, j) => j !== k && within(other.at, p) < sunk * other.r) || (!spec.open && Math.abs(p.z) < 0.75 && Math.hypot(p.x, p.y) < 0.55 * spec.shape!((p.z + 1) / 2))) : whole;
            parts.push(place(clumped ? rounded(geo, 0.3) : geo, { at: [0, 0, cz], scale: [rx, ry, rz] }));
        });
        if (spec.berries && !lod) {
            const inside = (p: V, own: number) => crown.some((other, k) => k !== own && other.at.distanceTo(v3(p.x / rx, p.y / ry, (p.z - cz) / rz)) < 0.75 * other.r);
            parts.push(...berries(parts.slice(-crown.length), spec.berries, rand, inside));
        }
        if (spec.heads && flecks && !lod) {
            const inside = (p: V, own: number) => crown.some((other, k) => k !== own && other.at.distanceTo(v3(p.x / rx, p.y / ry, (p.z - cz) / rz)) < other.r);
            parts.push(...heads(parts.slice(-crown.length), spec.heads, flecks, mid, rand, inside));
        }
        // A bunch hangs from the side or the underside of a lump, where no other lump hides it.
        for (let k = 0, tries = 0; flecks && lod === 0 && k < (spec.bunches ?? 0) && tries < 1000; tries++) {
            const { at, r } = crown[Math.floor(rand() * crown.length)];
            const a = 2 * Math.PI * rand(), up = -0.75 + 0.9 * rand(), level = Math.sqrt(1 - up * up);
            // A lump that swells is less deep between its tufts.
            const p = at.clone().addScaledVector(v3(level * Math.cos(a), level * Math.sin(a), up * tall), (clumped && spec.billows ? 0.72 : 0.97) * r);
            if (crown.some(other => other.at !== at && other.at.distanceTo(p) < other.r)) continue;
            const size = 0.05 + 0.025 * rand(), fruit = hex(flecks[0]);
            const bunch = paint(soft(lathe(BUNCH, 5, 3)), (q) => mid.clone().lerp(fruit, 0.25 + 0.6 * smooth(0.1, -0.7, q.z)));
            parts.push(place(bunch, { at: toTree(p), rot: [0, 0, a], scale: [0.26 * size, 0.26 * size, size] }));
            k++;
        }
        // Limbs run from the fork into the outer lumps. An open crown keeps them a level further out,
        // so its puffs never float, and a crooked one at every level, as its puffs stand far from the trunk.
        if (clumped) {
            for (const [n, limb] of spec.trunk && lod < 2 ? limbs.entries() : []) parts.push(tube((main(n) ? [0, 0.33, 0.67, 1] : [0, 0.5, 1]).map(limb), (main(n) ? 0.45 : 0.3) * spec.trunk, 0.2 * spec.trunk, 4, bark));
        } else if (spec.trunk && (lod === 0 || (spec.open && lod === 1) || spec.crook)) {
            for (const { at } of crown.slice(1)) {
                const end = toTree(at.clone().multiplyScalar(0.8));
                const from = leave(end, spec.stagger ? Math.max(spec.fork, end.z - 0.6 * Math.hypot(end.x, end.y)) : spec.fork);
                if (spec.crook) {
                    // The limb leaves the trunk nearly level and turns up under its puff.
                    const knee = from.clone().lerp(end, 0.6).setZ(lerp(from.z, end.z, 0.25));
                    parts.push(tube(lod < 2 ? [from, knee, end] : [from, end], spec.trunk * 0.45, spec.trunk * 0.3, [6, 4, 3][lod], bark));
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
function rods(spec: Broadleaf, lod: Lod, bare: boolean, foliage: (p: V, n: V) => Colour, [, tip]: Colour[], [catkin, share]: [Colour?, number?], rand: () => number, seed: number): Geometry[] {
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
            const geo = paint(soft(lathe([[0, 0], [1, 0.4], [0, 1]], 4, 2)), catkin!);
            parts.push(place(geo, { at: path(s).addScaledVector(out, width), turn: turnFromTo(UP, out.setZ(1.2).normalize()), scale: [0.5 * across, 0.5 * across, spec.heads ? across * tall : 0.065] }));
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
function dragon(spec: Broadleaf, lod: Lod, [deep, mid, tip]: Colour[], rand: () => number): Geometry[] {
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

/**
 * Swells a lump of radius r into `count` round tufts that stand out of a core, most of them on its upper side, as
 * balls that share one skin. Returns how far each vertex swells, 0 to 1.
 */
function billow(geo: Geometry, r: number, count: number, seed: number): number[] {
    const p = geo.position, d = v3(), swell: number[] = [];
    const tufts = Array.from({ length: count }, (_, k) => {
        const z = lerp(0.9, -0.4, (k + 0.5) / count), level = Math.sqrt(1 - z * z), a = k * GOLDEN + seed;
        return v3(level * Math.cos(a), level * Math.sin(a), z);
    });
    // A tuft is a ball of radius 0.5 r that stands 0.55 r out of the centre; the core has radius 0.7 r.
    for (let k = 0; k < p.length / 3; k++) {
        const far = Math.max(0.7, ...tufts.map(c => {
            const cos = d.from(p, k).normalize().dot(c), under = 0.25 - 0.3025 * (1 - cos * cos);
            return cos > 0 && under > 0 ? 0.55 * cos + Math.sqrt(under) : 0;
        }));
        d.multiplyScalar(r * far);
        p.splice(3 * k, 3, d.x, d.y, d.z);
        swell.push((far - 0.7) / 0.35);
    }
    computeNormals(geo);
    return swell;
}

/** Squashes a lump of radius r into a pad with a flat top and round sides. */
function pad(geo: Geometry, r: number): void {
    const p = geo.position;
    for (let i = 2; i < p.length; i += 3) p[i] -= 0.4 * r * smooth(0.2 * r, r, p[i]);
    computeNormals(geo);
}

/** Draws the underside of a lump of radius r down into a hanging point. */
function hang(geo: Geometry, r: number, amount: number): Geometry {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) {
        const k = smooth(0.2, -1, p[i + 2] / r);
        const narrow = 1 - 0.5 * amount * k;
        p.splice(i, 3, p[i] * narrow, p[i + 1] * narrow, p[i + 2] - amount * r * k * k);
    }
    return computeNormals(geo);
}

/** Shears a tree downwind, towards +x, more the higher it reaches. */
function sweep(geo: Geometry, amount: number): void {
    const p = geo.position;
    for (let i = 0; i < p.length; i += 3) p[i] += amount * p[i + 2] ** 2;
    computeNormals(geo);
}
