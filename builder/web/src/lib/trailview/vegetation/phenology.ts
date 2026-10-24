// The trees' year. Each tree passes four turns: leaf-out, full green, autumn colour and leaf fall. A
// stand passes a turn over `days` about its `day`, and each tree on a day of its own, by its hash: the
// share of trees past a turn rises with the date, and a tree never turns back. Higher up, spring comes
// later and autumn earlier.
import { TREES } from '../models/library';
import { Colour, hex, ramp } from '../models/shape';
import { SEASONS, type Region, type TreeRule } from '../rules/regions';

/** The phases between the turns, in the order of SEASONS: winter (bare), spring, summer, autumn. */
export const WINTER = 0, SUMMER = 2;

/**
 * As TURNS of phenology.wgsl, per turn: its day of the year at REFERENCE_M (0 is 1 January), the days
 * over which a stand passes it, and the days that it comes later per metre higher. A stand passes
 * each turn before any of its trees pass the next.
 */
export const TURNS: readonly (readonly [number, number, number])[] = [
    // Leaf-out, about 20 April.
    [110, 30, 0.03],
    // Full green, about 4 June.
    [155, 30, 0.03],
    // Autumn colour, about 29 September.
    [272, 40, -0.015],
    // Leaf fall, about 4 November.
    [308, 30, -0.015],
];
/** The elevation of the turns' days, and the highest trees: no tree turns as if it stood higher. */
export const REFERENCE_M = 500, TREELINE_M = 2500;

/** The share of the trees at an elevation that have passed turn k on a day, as `turned` of phenology.wgsl. */
export function turned(k: number, day: number, elevation: number): number {
    const [at, days, later] = TURNS[k];
    return Math.min(1, Math.max(0, (day - at - later * (Math.min(Math.max(elevation, 0), TREELINE_M) - REFERENCE_M)) / days + 0.5));
}

/** The phase of a tree whose hash is `u` (0 to 1), as `phase_of` of phenology.wgsl. */
export function phaseOf(u: number, day: number, elevation: number): number {
    let passed = 0;
    for (let k = 0; k < TURNS.length; k++) if (u < turned(k, day, elevation)) passed++;
    return passed % 4;
}

/** The share of the trees in each phase at an elevation, winter first. */
export function phaseShares(day: number, elevation: number): [number, number, number, number] {
    const [a, b, c, d] = [0, 1, 2, 3].map(k => turned(k, day, elevation));
    return [1 - a + d, a - b, b - c, c - d];
}

/** The phases that a tree from sea level to the tree line has on a day. */
export function phasesOn(day: number): Set<number> {
    const phases = new Set<number>();
    // A step of 50 m moves a turn by less than a day; its window is a month wide.
    for (let elevation = 0; elevation <= TREELINE_M; elevation += 50) {
        phaseShares(day, elevation).forEach((share, phase) => share > 0 && phases.add(phase));
    }
    return phases;
}

/** The elevations of the forest's colours (forestYear): every YEAR_STEP_M metres from 0, YEAR_KNOTS of them, as in ground.wgsl. */
export const YEAR_STEP_M = 300, YEAR_KNOTS = 10;
/** The leaf types of a woodland, as `leafOf` gives them and the forest's canopy codes: any, broadleaved, needleleaved, mixed. */
export const LEAVES = 4;
/** The phases whose colours the ground takes from the trees; in summer it keeps its own. */
const TURNED = [WINTER, 1, 3];

/** The leaf type of a woodland material, as LEAVES orders them. */
export const leafOf = (material: string) => ['forest', 'broadleaf-forest', 'needleleaf-forest', 'mixed-forest'].indexOf(material);

/**
 * A region's forest per leaf type (LEAVES), elevation (YEAR_STEP_M) and phase (winter, spring and
 * autumn): the mean crown colour of the trees that stand there, in linear light, per unit of the luma
 * of their mean in summer. The trees are those that the forest picks (forest.wgsl): of the leaf type in
 * their band, else in their band, else of the leaf type, else all, by weight; a mixed wood half of
 * each leaf type. ground.wgsl gives a forest texel these colours at its own luma, in the share of the
 * trees in each phase.
 */
export function forestYear(region: Region): number[][] {
    const trees = region.trees.filter(rule => rule.cover === 'tree' && rule.model in TREES && rule.weight > 0);
    const kinds = ['broadleaved', 'needleleaved'];
    // The trees of a leaf type (0 any) at an elevation, by weight.
    const stand = (leaf: number, e: number) => {
        const rules = [(t: TreeRule) => inBand(t, e) && t.leaf === kinds[leaf - 1], (t: TreeRule) => inBand(t, e), (t: TreeRule) => t.leaf === kinds[leaf - 1], () => true];
        const first = rules.find(rule => trees.some(rule));
        return first ? trees.filter(first).map(t => ({ t, w: t.weight })) : [];
    };
    const weighed = (list: { t: TreeRule; w: number }[]) => list.map(({ t, w }) => ({ t, w: w / list.reduce((sum, x) => sum + x.w, 0) }));
    return Array.from({ length: LEAVES }, (_, leaf) => Array.from({ length: YEAR_KNOTS }, (_, k) => {
        const e = k * YEAR_STEP_M;
        const list = leaf === 3 ? [...weighed(stand(1, e)), ...weighed(stand(2, e))] : stand(leaf, e);
        const mean = (phase: number) => [0, 1, 2].map(c => list.reduce((sum, { t, w }) => sum + w * crown(t.model, t.evergreen ? SUMMER : phase)[c], 0));
        const summer = mean(SUMMER), luma = 0.3 * summer[0] + 0.59 * summer[1] + 0.11 * summer[2];
        return TURNED.map(phase => (luma > 0 ? mean(phase).map(c => c / luma) : [1, 1, 1]));
    }).flat()).flat();
}

const inBand = ({ elevation: [low, high] }: TreeRule, e: number) => e >= low && e <= high;

/** The mean colour of a tree model's crown in a phase, in linear light: its ramp, or its twigs when bare, and its flecks. */
function crown(model: string, phase: number): number[] {
    const spec = TREES[model], season = SEASONS[phase], own = spec.form === 'conifer' ? spec.needles[season] : spec.leaves[season];
    const [deep, mid, tip] = ramp(own === 'bare' ? spec.twigs! : own), fleck = spec.form === 'broadleaf' && spec.flecks?.[season];
    const mean = new Colour((deep.r + mid.r + tip.r) / 3, (deep.g + mid.g + tip.g) / 3, (deep.b + mid.b + tip.b) / 3);
    if (fleck) mean.lerp(hex(fleck[0]), fleck[1]);
    return [mean.r, mean.g, mean.b];
}
