import type { Gpu } from '../gpu/device';
import type { Instances } from '../instances/instances';
import { packPoints, Points } from '../instances/points';
import { TREES } from '../models/library';
import { SEASONS, type Rules, type Season } from '../rules/regions';
import type { FeatureTiles } from '../terrain/stamps';
import type { BuiltMesh } from './plant-worker';

/** Orchard trees stand within this many metres of the eye, and the pool holds this many. */
const REACH_M = 2000, CAPACITY = 8192;
/** The tiers of an orchard tree, as the forest's trees, and its heights in metres. */
const TIERS_M = [30, 150, 700], HEIGHT_M: [number, number] = [4.5, 6];
const ZOOM = 14, EARTH_M = 40075016.686;
/** Feature tiles kept, and loads in flight at most. */
const KEPT = 64, LOADS = 4;

/**
 * The orchards of L8's kind `orchard`, in rows (orchard-rows.ts) of the tree model that the pack of the
 * region at the eye names, as point instances. A worker makes the trees of each zoom 14 feature tile
 * about the eye.
 */
export class Orchards {
    private readonly pool: Points;
    private readonly models = new Map<string, number>();
    private readonly worker = new Worker(new URL('./orchard-worker.ts', import.meta.url), { type: 'module' });
    private readonly builder = new Worker(new URL('./plant-worker.ts', import.meta.url), { type: 'module' });
    private readonly tiles = new Map<string, Float64Array | null>();
    private readonly waiting = new Map<number, (trees: Float64Array | null) => void>();
    private readonly pending = new Set<string>();
    private next = 0;
    private season?: Season;
    private building = 0;
    private drawn = '';
    /** Whether the last update changed what the orchards draw, other than by the camera. */
    newContent = false;

    constructor(rules: Rules, instances: Instances, private readonly features: FeatureTiles | undefined, private readonly wake: () => void) {
        this.pool = new Points(instances, CAPACITY);
        for (const model of new Set(rules.regions.flatMap(region => (region.orchard && region.orchard in TREES ? [region.orchard] : [])))) {
            this.models.set(model, instances.add({ tiers: [] }));
        }
        this.worker.onmessage = ({ data }) => {
            const resolve = this.waiting.get(data.id);
            this.waiting.delete(data.id);
            if (data.error) console.warn(`Orchard tile: ${data.error}`);
            resolve?.(data.error ? null : data.trees);
        };
        this.builder.onmessage = ({ data }: MessageEvent<{ built?: BuiltMesh[][]; error?: string }>) => {
            this.building--;
            if (data.error || !data.built) return void console.warn(`Orchard models: ${data.error}`);
            [...this.models.values()].forEach((model, i) => instances.replace(model, {
                tiers: data.built![i].map((mesh, t) => ({ mesh, until: TIERS_M[t] })), impostor: { from: 1, until: REACH_M },
            }));
            this.newContent = true;
            this.drawn = '';
            wake();
        };
    }

    get idle(): boolean {
        return this.pending.size === 0 && this.building === 0;
    }

    attach(gpu: Gpu, frame: GPUBuffer, surface: { layout: GPUBindGroupLayout; group: GPUBindGroup }, features: { layout: GPUBindGroupLayout; group: GPUBindGroup }): Promise<void> {
        this.season = undefined;
        this.drawn = '';
        return this.pool.attach(gpu, frame, surface, features);
    }

    /** Asks for the feature tiles about the eye at (lon, lat), and puts the trees of the known ones in the pool, of the model of `region`. */
    update(lonLat: [number, number], date: string, region: { orchard?: string } | undefined): void {
        this.newContent = false;
        if (!this.features || !this.models.size) return;
        const season = SEASONS[Math.floor((Number(date.slice(5, 7)) - 1) / 3)];
        if (season !== this.season) {
            this.season = season;
            this.building++;
            this.builder.postMessage({ id: 0, models: [...this.models.keys()].map(model => ({ model, tree: true, season, lods: [0, 1, 2] })) });
        }
        const n = 2 ** ZOOM, ex = (lonLat[0] + 180) / 360 * n, lat = lonLat[1] * Math.PI / 180;
        const ey = (1 - Math.log(Math.tan(lat) + 1 / Math.cos(lat)) / Math.PI) / 2 * n;
        const reach = REACH_M / (EARTH_M * Math.cos(lat) / n), wanted: string[] = [];
        for (let y = Math.floor(ey - reach); y <= Math.floor(ey + reach); y++) for (let x = Math.floor(ex - reach); x <= Math.floor(ex + reach); x++) {
            const name = `${x}/${y}`;
            wanted.push(name);
            if (this.tiles.has(name) || this.pending.has(name) || this.pending.size >= LOADS) continue;
            this.pending.add(name);
            this.load(x, y).then(trees => this.keep(name, trees), () => this.keep(name, null)).finally(() => {
                this.pending.delete(name);
                this.wake();
            });
        }
        const name = region?.orchard && this.models.has(region.orchard) ? region.orchard : this.models.keys().next().value!;
        const model = this.models.get(name)!, windward = !!TREES[name].keepsHeading;
        const known = wanted.filter(name => this.tiles.get(name)), drawn = `${model} ${known.join(' ')}`;
        if (drawn === this.drawn) return;
        this.drawn = drawn;
        const points = known.flatMap(name => {
            const trees = this.tiles.get(name)!;
            return Array.from({ length: trees.length / 2 }, (_, i) => {
                const x = trees[2 * i], y = trees[2 * i + 1], h = ((x * 73856093) ^ (y * 19349663)) >>> 0;
                return { x, y, yaw: windward ? 0 : (h % 628) / 100, height: HEIGHT_M[0] + ((h >>> 10) % 100) / 100 * (HEIGHT_M[1] - HEIGHT_M[0]), model, tint: 0.9 + ((h >>> 17) % 20) / 100 };
            });
        });
        this.pool.write(packPoints(points));
        this.newContent = true;
    }

    private async load(x: number, y: number): Promise<Float64Array | null> {
        const body = await this.features!(ZOOM, x, y);
        if (!body) return null;
        const id = this.next++;
        return new Promise(resolve => {
            this.waiting.set(id, resolve);
            this.worker.postMessage({ id, tile: { x, y }, body }, [body]);
        });
    }

    private keep(name: string, trees: Float64Array | null): void {
        this.tiles.set(name, trees && trees.length ? trees : null);
        for (const old of this.tiles.keys()) {
            if (this.tiles.size <= KEPT) break;
            this.tiles.delete(old);
        }
    }
}
