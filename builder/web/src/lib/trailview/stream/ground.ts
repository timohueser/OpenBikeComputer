import type { PlannerConfig } from '../../planner/config';
import { decodedTiles, openSource, requestSignal, type Source, type TileGetter, type Tiles } from '../../planner/layers/archive';
import { snowMeta } from '../../planner/layers/snow';
import { releaseFetch } from '../../planner/release';
import type { GroundResult } from '../ground/pages';
import { rulesOf, type Rules } from '../rules/regions';
import { snowDay, type GroundDate } from './ground-pages';
import type { Body, GroundJob } from './ground-worker';

/** The archives of the release (specs/trailview-tiles.md), by their names in the catalogue's `layers`. */
const NAMES = {
    material: 'trailview-material', lithology: 'trailview-lithology', region: 'trailview-region', ndvimax: 'trailview-colour-ndvimax',
    q1: 'trailview-colour-q1', q2: 'trailview-colour-q2', q3: 'trailview-colour-q3', q4: 'trailview-colour-q4', snow: 'snow',
} as const;
type Layer = keyof typeof NAMES;
const QUARTERS: Layer[] = ['q1', 'q2', 'q3', 'q4'];
const WORKERS = 2;
/** Tile bodies kept per layer, so a new device or a page that comes back builds again without a request. */
const BODIES = 16 * 2 ** 20;
const SNOW_TILE = 256 * 256 * 2;

/**
 * The ground of a release: the ruleset, the archives of the ground, colour and snow layers, and the
 * workers that build the ground pages of a material tile on a date.
 */
export class GroundSource {
    requests = 0;
    bytes = 0;
    failed = 0;
    private readonly bodies: Partial<Record<Layer, Tiles<ArrayBuffer>>>;
    /** The snow days of one day step: a new day loads the snow tiles again. */
    private snow?: { index: number; tiles: Tiles<ArrayBuffer> };
    private readonly workers = Array.from({ length: WORKERS }, () => new Worker(new URL('./ground-worker.ts', import.meta.url), { type: 'module' }));
    private readonly waiting = new Map<number, { worker: Worker; resolve: (data: GroundResult) => void; reject: (error: Error) => void }>();
    private next = 0;

    private constructor(readonly rules: Rules, private readonly sources: Partial<Record<Layer, Source>>, readonly seasons: number) {
        this.bodies = Object.fromEntries(Object.keys(sources).filter(layer => layer !== 'snow')
            .map(layer => [layer, decodedTiles(this.counted(layer as Layer), body => body.buffer, BODIES, body => body.byteLength)]));
        for (const worker of this.workers) {
            worker.onmessage = ({ data }) => {
                const wait = this.waiting.get(data.id);
                this.waiting.delete(data.id);
                if (data.error) wait?.reject(new Error(data.error));
                else wait?.resolve(data);
            };
            // A worker that fails or sends what does not arrive fails the jobs it holds.
            worker.onerror = worker.onmessageerror = () => {
                for (const [id, wait] of this.waiting) {
                    if (wait.worker !== worker) continue;
                    this.waiting.delete(id);
                    wait.reject(new Error('The ground worker failed.'));
                }
            };
        }
    }

    /**
     * The ground of a config; undefined when it has no ruleset or no material archive. Only the rules
     * and the material fail it: another archive that does not open is left out.
     */
    static async open(config: PlannerConfig): Promise<GroundSource | undefined> {
        const urls = config.layers as Partial<Record<string, string>>;
        if (!config.trailview_rules || !urls[NAMES.material]) return undefined;
        const response = await releaseFetch(config.trailview_rules, { signal: requestSignal() });
        if (!response.ok) throw new Error(`The TrailView rules answered ${response.status}.`);
        const rules = rulesOf(await response.json());
        let seasons = 0;
        const entries = await Promise.all(Object.entries(NAMES).map(async ([layer, name]) => {
            const url = urls[name];
            if (!url) return [layer, undefined] as const;
            const opened = openSource(url).then(source => {
                if (layer === 'snow') seasons = snowMeta(source.metadata).seasons;
                return source;
            });
            return [layer, await (layer === 'material' ? opened : opened.catch(error => void console.warn(`TrailView leaves out ${name}: ${error instanceof Error ? error.message : error}`)))] as const;
        }));
        return new GroundSource(rules, Object.fromEntries(entries.filter(([, source]) => source)), seasons);
    }

    /** The finest level with ground pages: one finer than the material's last zoom. */
    get finest(): number {
        return this.sources.material!.maxZoom + 1;
    }

    /** The credits of the layers. */
    get attribution(): string[] {
        return [...new Set(Object.values(this.sources).map(source => String(source.metadata.attribution ?? '')).filter(Boolean))];
    }

    /** The coarsest level whose pages carry the satellite colour. */
    get coloured(): number {
        return (this.sources.q1?.minZoom ?? Infinity) + 1;
    }

    /** The colour variant of the release, from the colour metadata. */
    get colour(): string {
        return String(this.sources.q1?.metadata.variant ?? 'none');
    }

    /**
     * The four pages of material tile z/x/y on a date, with the two quarters around it; null where it
     * has no ground. A failed material tile fails them; a failed tile of another layer leaves that
     * layer out, and the pages are not complete. Without `fine`, pages whose four snow tiles one zoom
     * finer have not arrived take the one snow tile of zoom z, and are `coarse` where finer snow can follow.
     */
    async pages(z: number, x: number, y: number, quarters: [number, number], date: GroundDate, world: number, fine = false): Promise<GroundResult> {
        let complete = true;
        const optional = (body: Promise<Body | undefined>) => body.catch(() => void (complete = false));
        const at = (layer: Layer, zoom: number, planar = false) => {
            const body = this.body(layer, Math.min(zoom, this.sources[layer]?.maxZoom ?? -1), x, y, z, planar, date.snowIndex);
            return layer === 'material' ? body : optional(body);
        };
        const colour = this.sources.q1, zc = Math.min(z, colour?.maxZoom ?? -1);
        const coloured = colour !== undefined && zc >= colour.minZoom;
        const snow = this.sources.snow, zs = z + 1;
        const finer = snow !== undefined && zs >= snow.minZoom && zs <= snow.maxZoom ? [0, 1, 2, 3].map(q => [2 * x + (q & 1), 2 * y + (q >> 1)]) : [];
        const days = snow && this.snowDays(date.snowIndex);
        const coarse = finer.length > 0 && !fine && finer.some(([sx, sy]) => days!.loaded(zs, sx, sy) === undefined);
        const snowBodies = !snow ? [] : finer.length && !coarse
            ? finer.map(([sx, sy]) => optional(this.body('snow', zs, sx, sy, zs, true, date.snowIndex)))
            : [at('snow', z, true)];
        const [material, lithology, region, a, b, ndvimax, ...snowTiles] = await Promise.all([
            at('material', z), at('lithology', z), at('region', z),
            ...(coloured ? [at(QUARTERS[quarters[0]], zc), at(QUARTERS[quarters[1]], zc), at('ndvimax', zc)] : [undefined, undefined, undefined]),
            ...snowBodies,
        ]);
        const job: GroundJob = {
            z, x, y, material, lithology, region, quarters: coloured ? [a, b] : undefined, ndvimax, snow: snowTiles, date, world,
        };
        const id = this.next++, worker = this.workers[id % WORKERS];
        const bodies = [material, lithology, region, a, b, ndvimax, ...snowTiles].flatMap(body => (body ? [body.body] : []));
        const built = await new Promise<GroundResult>((resolve, reject) => {
            this.waiting.set(id, { worker, resolve, reject });
            worker.postMessage({ id, job }, bodies);
        });
        // Where the snow tile of zoom z is absent, the finer ones are too, unless zoom z is coarser than the snow's first zoom.
        const refines = coarse && (snowTiles[0] !== undefined || z < snow!.minZoom);
        return { pages: built.pages, complete: complete && built.complete, coarse: refines };
    }

    /** The body of the tile of a layer at a zoom that holds tile z/x/y of zoom `of`; undefined where the layer has none. */
    private async body(layer: Layer, zoom: number, x: number, y: number, of: number, planar = false, index = 0): Promise<Body | undefined> {
        const source = this.sources[layer];
        if (!source || zoom < source.minZoom || zoom > of) return undefined;
        const tx = x >> (of - zoom), ty = y >> (of - zoom);
        const body = await (layer === 'snow' ? this.snowDays(index) : this.bodies[layer]!).tile(zoom, tx, ty);
        // A copy for the worker to take: the bodies stay here.
        return body ? { key: `${layer}/${zoom}/${tx}/${ty}`, z: zoom, x: tx, y: ty, body: body.slice(0), planar } : undefined;
    }

    /** The snow days of a day step, kept in place of the snow tiles. */
    private snowDays(index: number): Tiles<ArrayBuffer> {
        if (this.snow?.index !== index) {
            const tiles = decodedTiles(this.counted('snow'), body => {
                if (body.byteLength !== this.seasons * SNOW_TILE) throw new Error(`A snow tile of ${this.seasons} seasons has ${body.byteLength} bytes.`);
                return snowDay(body, this.seasons, index).buffer;
            }, BODIES, day => day.byteLength);
            this.snow = { index, tiles };
        }
        return this.snow.tiles;
    }

    /** The tiles of a layer, counted in the requests, the bytes and the failures. */
    private counted(layer: Layer): TileGetter {
        const source = this.sources[layer]!;
        return async (z, x, y, signal) => {
            try {
                const body = await source.get(z, x, y, signal);
                this.requests++;
                this.bytes += body?.byteLength ?? 0;
                return body;
            } catch (error) {
                this.failed++;
                throw error;
            }
        };
    }
}
