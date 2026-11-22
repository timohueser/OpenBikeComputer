// The model files of the build (specs/trailview-models.md): every model of one fixed shape, made by its generator and
// packed (pack.ts). vite/model-files.ts runs this module in Node and serves what it returns.
import { KINDS } from '../kinds';
import { SEASONS, type Season } from '../rules/regions';
import { SUMMER } from '../vegetation/phenology';
import { ANIMALS } from './animals';
import { geometry } from './flat';
import { bareIn, tree, TREES } from './library';
import type { FloatMesh, Models, Packed } from './model-file';
import { encodeModels, pack } from './pack';
import { plant, PLANTS } from './plant-library';
import { rock } from './rocks';
import { mesh, Sculpt } from './sculpt';
import { lit, relit, simplified, simplifierReady } from './simplify';
import { fits, tone } from './tone';
import type { Grown, Lod, Tree } from './trees';

/** The crown triangles of tree tiers 1 and 2 that the simplifier makes. */
const TRIANGLES = [600, 200];
/** A phase draws the summer crown through a colour matrix where that turns every crown vertex within this many sRGB steps of 255. */
const FIT = 8;
/** Shapes of each plant, and rocks of each kind: as SEEDS of plants.ts and SLABS of dressing.ts. */
const SEEDS = 3, SLABS = 3;

/**
 * The model budget of specs/trailview-models.md: per kind of model and mesh tier, the most triangles and vertices; and
 * the most triangles of tier 0 of a summer crown, as the mean of every tree species. Measured on the models of develop
 * 4a5419013, the largest counts rounded up; the owner chose about 5 times for the animals and 4 times for the small
 * objects, which few instances draw.
 */
const BUDGET: Record<string, [number, number][]> = {
    tree: [[4700, 9000], [900, 1800], [230, 600]],
    plant: [[350, 1000]],
    animal: [[1000, 3000], [250, 750]],
    rock: [[100, 300]],
    object: [[600, 1800]],
};
const TREE_MEAN = 3400;

/** What the build serves: each file by name, and the models with the names of their files; and the budget, for the review page. */
export interface Baked { files: Map<string, Uint8Array>; models: Models; budget: typeof BUDGET }

export async function bake(): Promise<Baked> {
    await simplifierReady;
    const files = new Map<string, Uint8Array>(), over: string[] = [], summers: number[] = [];
    // Models of one kind keep the budget of that kind.
    const check = (kind: string, name: string, models: Packed[]) => models.forEach((model, m) => model.tiers.forEach(({ vertices, index }, t) => {
        const cap = BUDGET[kind][t], triangles = index.length / 3, count = vertices.length / 16;
        if (!cap || triangles > cap[0] || count > cap[1]) over.push(`${name} model ${m} tier ${t}: ${triangles} triangles, ${count} vertices, the budget ${cap ? cap.join(' and ') : 'has no such tier'}`);
    }));
    const file = async (kind: string, name: string, models: Packed[]) => (check(kind, name, models), files.set(name, await encodeModels(models)), name);
    const models: Models = { trees: {}, plants: {}, breeds: {}, objects: { file: '', rocks: 2 * SLABS, points: [] } };

    for (const [id, spec] of Object.entries(TREES)) {
        const summer = ([0, 1, 2] as Lod[]).map(lod => tree(id, 'summer', lod));
        const tones: (number[] | null)[] = [], crowns: (string | null)[] = [];
        for (const [phase, season] of SEASONS.entries()) {
            if (phase === SUMMER) {
                tones.push(null);
                crowns.push(null);
                continue;
            }
            const near = bareIn(id, season) ? undefined : tree(id, season, 0), matrix = near && tone(summer[0], near);
            tones.push(matrix ?? null);
            crowns.push(!matrix || !fits(matrix, summer[0], near!, FIT) ? await file('tree', `tree-${id}-${season}`, [crown(id, season)]) : null);
        }
        const leafy = treeModel(tiers(id, 'summer', summer));
        summers.push(leafy.tiers[0].index.length / 3);
        models.trees[id] = { summer: await file('tree', `tree-${id}-summer`, [leafy]), keepsHeading: !!spec.keepsHeading, tones, crowns };
    }

    for (const [id, { ground }] of Object.entries(PLANTS)) {
        const seasons: Record<string, string> = {};
        for (const season of SEASONS) seasons[season] = await file('plant', `plant-${id}-${season}`, Array.from({ length: SEEDS }, (_, k) => pack([float32(plant(id, season, k + 1))])));
        models.plants[id] = { ground, seasons };
    }

    // A breed's models, each coat in each pose, with a near and a far tier.
    for (const [name, animal] of Object.entries(ANIMALS)) {
        const built = animal.coats.flatMap(coat => Array.from({ length: animal.poses }, (_, pose) => pack([0, 1].map(tier => {
            const s = new Sculpt(tier);
            animal.build(s, coat, pose);
            return mesh(s.model);
        }))));
        models.breeds[name] = { height: animal.height, coats: animal.coats.length, poses: animal.poses, file: await file('animal', `animal-${name}`, built) };
    }

    const rocks = (['slab', 'boulder'] as const).flatMap(name => Array.from({ length: SLABS }, (_, k) => pack([geometry(rock(name, k + 1))])));
    const points = [...KINDS].flatMap(([, kind]) => Object.values(kind.models ?? {}).map(source => source()));
    const objects = points.map(({ tiers }) => pack(tiers.map(({ mesh }) => mesh)));
    check('rock', 'objects', rocks);
    check('object', 'objects', objects);
    files.set('objects', await encodeModels([...rocks, ...objects]));
    models.objects = { file: 'objects', rocks: rocks.length, points: points.map(({ tiers, casts }) => ({ until: tiers.map(({ until }) => until), casts })) };
    const mean = summers.reduce((sum, n) => sum + n, 0) / summers.length;
    if (mean > TREE_MEAN) over.push(`the tree species: ${Math.round(mean)} triangles in tier 0 on average, the budget ${TREE_MEAN}`);
    if (over.length) throw new Error(`Models over the budget of specs/trailview-models.md:\n${over.join('\n')}`);
    return { files, models, budget: BUDGET };
}

/** A crown of a species' own in a season: bare crowns grow each tier from the tree, the others take theirs from `tiers`. */
function crown(id: string, season: Season): Packed {
    const grown = ([0, 1, 2] as Lod[]).map(lod => tree(id, season, lod));
    return treeModel(bareIn(id, season) ? grown : tiers(id, season, grown));
}

/**
 * How a species' leafy crown gets tiers 1 and 2, each with its own trunk and as bright as tier 0. A crown of rosettes of
 * cards, which simplify into shards, grows both. Thin limbs, sprays or fronds would come apart, so tier 2 grows and
 * tier 1 is simplified from tier 0. A shaped crown of many small lumps would come out in shards from tier 0, so tier 2
 * is simplified from its own far level. Any other crown simplifies both from tier 0.
 */
function farTiers(spec: Tree): 'grown' | 'far grown' | 'far simplified' | 'simplified' {
    if (spec.form === 'broadleaf' && spec.branching) return 'grown';
    if (spec.form === 'conifer' ? spec.boughs || spec.tufts : (spec.crook && !spec.shape) || spec.fronds) return 'far grown';
    return spec.form === 'broadleaf' && spec.shape ? 'far simplified' : 'simplified';
}

/** Tier 0 and the tiers that `farTiers` makes; the simplifier takes tier 0 without its flower heads. */
function tiers(id: string, season: Season, [near, middle, far]: Grown[]): Grown[] {
    const want = lit(near), how = farTiers(TREES[id]);
    if (how === 'grown') return [near, relit(middle, want), relit(far, want)];
    // Flower heads simplify into loose shards; the crown's colour keeps theirs.
    const source = tree(id, season, 0, 1, false);
    return [near, simplified(source, middle, TRIANGLES[0], want), how === 'far grown' ? relit(far, want) : simplified(how === 'far simplified' ? far : source, far, TRIANGLES[1], want)];
}

/** A tree model, with its crown's lowest point at every tier. */
const treeModel = (grown: Grown[]) => pack(grown.map(geo => ({ ...float32(geo), trunk: geo.trunk })), Math.min(...grown.map(geo => geo.crown)));

/** A tree's or a plant's mesh as the device made it: float32 arrays, without glow or rise. */
const float32 = (geo: FloatMesh): FloatMesh => ({ position: Float32Array.from(geo.position), normal: Float32Array.from(geo.normal), color: Float32Array.from(geo.color), index: Uint32Array.from(geo.index) });
