// The tree builder, the prototype's src/models/trees.ts without three.js: a species file in trees/
// says what a tree is, and this draws it with the forms of trees/form/ at three levels of detail. A
// tree is 1 tall with its trunk base at the origin and z up; the instance shader scales it by the
// tree's height. The season changes vertex colours, and a bare tree's crown.

import type { Season } from '../rules/regions';
import { unit, type Geometry, type Ramp } from './shape';
import { broadleaf } from './trees/form/broadleaf';
import { conifer } from './trees/form/conifer';

/** 0 the most detail; each level has fewer triangles. */
export type Lod = 0 | 1 | 2;

export interface Bark {
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

export interface Conifer extends Base {
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

export interface Broadleaf extends Base {
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

/** Without `heads`, a broadleaf grows no flower heads, but its crown keeps their colour. */
export function grow(spec: Tree, season: Season, lod: Lod, seed: number, heads = true): Grown {
    const { geo, crown, trunk } = spec.form === 'conifer' ? conifer(spec, season, lod, seed) : broadleaf(spec, season, lod, seed, heads);
    // No crown reaches under the ground: low boughs and lumps that would lie on it. Their normals stay, so they light as before.
    const p = geo.position;
    for (let k = 3 * trunk + 2; k < p.length; k += 3) p[k] = Math.max(0, p[k]);
    const top = Math.max(...p.filter((_, i) => i % 3 === 2));
    return Object.assign(unit(geo), { trunk, crown: Math.max(0, crown / top) });
}

/** A tree and, as in `Grown`, its trunk's vertices first and its crown's lowest point, before `unit`. */
export interface Built { geo: Geometry; trunk: number; crown: number }
