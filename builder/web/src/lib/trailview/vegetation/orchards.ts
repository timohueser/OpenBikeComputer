import { worldPixel } from '../../planner/layers/mercator';
import { UNITS } from '../frame/space';
import type { Instances } from '../instances/instances';
import { packPoints, Points, type Point } from '../instances/points';
import { TREES } from '../models/library';
import type { Region } from '../rules/regions';
import { DEEPEST } from '../stream/features';
import type { FeatureTiles } from '../terrain/stamps';
import type { TreeModels } from './tree-models';

/** Orchard tiles load within this many metres of the eye, and the pool holds this many trees, the nearest. */
const REACH_M = 2000, CAPACITY = 8192;
/** An orchard tree's heights in metres. */
const HEIGHT_M: [number, number] = [4.5, 6];
/** Each zoom 14 tile takes its orchards from its quarter of the zoom 13 feature tile. */
const ZOOM = 14, EARTH_M = 40075016.686;
/** Feature tiles kept, and loads in flight at most. */
const KEPT = 64, LOADS = 4;
/** World units per zoom 14 tile, and the step of the eye that sorts a full pool again. */
const TILE_UNITS = UNITS / 2 ** ZOOM, RESORT_UNITS = TILE_UNITS / 4;

/**
 * The orchards of L8's kind `orchard`, in rows (orchard-rows.ts), as point instances. A worker makes
 * the trees of each zoom 14 tile about the eye; a tile's trees are of the tree model that the
 * pack of the region at the tile names, the forest's model of that species (tree-models.ts), and a
 * tile in a region whose pack names none has no trees.
 */
export class Orchards {
    private readonly pool: Points;
    /** The instance model of each region's orchard tree. */
    private readonly models = new Map<number, { model: number; windward: boolean }>();
    private readonly worker = new Worker(new URL('./orchard-worker.ts', import.meta.url), { type: 'module' });
    private readonly tiles = new Map<string, Float64Array | null>();
    private readonly waiting = new Map<number, (trees: Float64Array | null) => void>();
    private readonly pending = new Set<string>();
    private next = 0;
    private drawn = '';
    private warned = false;
    /** Whether the last update changed what the orchards draw, other than by the camera. */
    newContent = false;

    constructor(rules: { regions: Region[] }, trees: TreeModels, instances: Instances, private readonly features: FeatureTiles | undefined, private readonly wake: () => void) {
        this.pool = new Points(instances, CAPACITY);
        rules.regions.forEach((region, r) => {
            if (!region.orchard || !(region.orchard in TREES)) return;
            const model = trees.index(region.orchard, region.trees.find(rule => rule.model === region.orchard)?.evergreen);
            if (model !== undefined) this.models.set(r, { model, windward: !!TREES[region.orchard].keepsHeading });
        });
        this.worker.onmessage = ({ data }) => {
            const resolve = this.waiting.get(data.id);
            this.waiting.delete(data.id);
            if (data.error) console.warn(`Orchard tile: ${data.error}`);
            resolve?.(data.error ? null : data.trees);
        };
    }

    get idle(): boolean {
        return this.pending.size === 0;
    }

    /**
     * Asks for the feature tiles about the eye at (lon, lat), and puts the trees of the known ones in
     * the pool, each tile's of the model of the region at its middle (`regionAt`).
     */
    update(lonLat: [number, number], regionAt: (lon: number, lat: number) => number | undefined): void {
        this.newContent = false;
        if (!this.features || !this.models.size) return;
        const n = 2 ** ZOOM, ex = (lonLat[0] + 180) / 360 * n, lat = lonLat[1] * Math.PI / 180;
        const ey = (1 - Math.log(Math.tan(lat) + 1 / Math.cos(lat)) / Math.PI) / 2 * n;
        const reach = REACH_M / (EARTH_M * Math.cos(lat) / n), wanted: string[] = [];
        for (let y = Math.floor(ey - reach); y <= Math.floor(ey + reach); y++) for (let x = Math.floor(ex - reach); x <= Math.floor(ex + reach); x++) {
            const name = `${x}/${y}`;
            wanted.push(name);
            if (this.tiles.has(name) || this.pending.has(name) || this.pending.size >= LOADS) continue;
            this.pending.add(name);
            this.load(x, y).then(trees => this.keep(name, trees, ex, ey), () => this.keep(name, null, ex, ey)).finally(() => {
                this.pending.delete(name);
                this.wake();
            });
        }
        const known = wanted.flatMap(name => {
            const trees = this.tiles.get(name), [x, y] = name.split('/').map(Number);
            if (!trees) return [];
            const region = regionAt((x + 0.5) / n * 360 - 180, Math.atan(Math.sinh(Math.PI * (1 - 2 * (y + 0.5) / n))) * 180 / Math.PI);
            const model = region === undefined ? undefined : this.models.get(region);
            return model ? [{ name, trees, ...model }] : [];
        });
        const count = known.reduce((sum, { trees }) => sum + trees.length / 2, 0), [ux, uy] = worldPixel(lonLat, UNITS);
        // A full pool keeps the nearest trees, sorted again as the eye moves.
        const drawn = `${known.map(({ name, model }) => `${name}:${model}`).join(' ')}${count > CAPACITY ? ` ${Math.round(ux / RESORT_UNITS)}/${Math.round(uy / RESORT_UNITS)}` : ''}`;
        if (drawn === this.drawn) return;
        this.drawn = drawn;
        let points: (Point & { far: number })[] = known.flatMap(({ trees, model, windward }) => Array.from({ length: trees.length / 2 }, (_, i) => {
            const x = trees[2 * i], y = trees[2 * i + 1], h = ((x * 73856093) ^ (y * 19349663)) >>> 0;
            return {
                x, y, yaw: windward ? 0 : (h % 628) / 100, height: HEIGHT_M[0] + ((h >>> 10) % 100) / 100 * (HEIGHT_M[1] - HEIGHT_M[0]), model,
                tint: 0.9 + ((h >>> 17) % 20) / 100, far: Math.hypot(x - ux, y - uy),
            };
        }));
        if (points.length > CAPACITY) {
            if (!this.warned) console.warn(`The orchards about the eye have ${points.length} trees; the nearest ${CAPACITY} stand.`);
            this.warned = true;
            points = points.sort((a, b) => a.far - b.far).slice(0, CAPACITY);
        }
        this.pool.write(packPoints(points));
        this.newContent = true;
    }

    private async load(x: number, y: number): Promise<Float64Array | null> {
        const up = ZOOM - DEEPEST, tile = { level: DEEPEST, x: x >> up, y: y >> up }, body = await this.features!(tile.level, tile.x, tile.y);
        if (!body) return null;
        const id = this.next++;
        return new Promise(resolve => {
            this.waiting.set(id, resolve);
            this.worker.postMessage({ id, tile, owned: { level: ZOOM, x, y }, body }, [body]);
        });
    }

    /** Keeps a tile's trees; past KEPT tiles, those farthest from the eye's tile (ex, ey) leave. */
    private keep(name: string, trees: Float64Array | null, ex: number, ey: number): void {
        this.tiles.set(name, trees && trees.length ? trees : null);
        if (this.tiles.size <= KEPT) return;
        const far = (key: string) => {
            const [x, y] = key.split('/').map(Number);
            return Math.hypot(x + 0.5 - ex, y + 0.5 - ey);
        };
        for (const old of [...this.tiles.keys()].sort((a, b) => far(b) - far(a)).slice(0, this.tiles.size - KEPT)) this.tiles.delete(old);
    }
}
