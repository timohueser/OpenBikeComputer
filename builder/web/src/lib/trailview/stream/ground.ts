import type { PlannerConfig } from '../../planner/config';
import { openSource, requestSignal, type Source } from '../../planner/layers/archive';
import { snowMeta } from '../../planner/layers/snow';
import { releaseFetch } from '../../planner/release';
import { rulesOf, type Rules } from '../rules/regions';
import type { GroundDate, GroundPage } from './ground-pages';
import type { Body, GroundJob } from './ground-worker';

/** The archives of the release (specs/trailview-tiles.md), by their names in the catalogue's `layers`. */
const NAMES = {
    material: 'trailview-material', lithology: 'trailview-lithology', region: 'trailview-region', ndvimax: 'trailview-colour-ndvimax',
    q1: 'trailview-colour-q1', q2: 'trailview-colour-q2', q3: 'trailview-colour-q3', q4: 'trailview-colour-q4', snow: 'snow',
} as const;
type Layer = keyof typeof NAMES;
const QUARTERS: Layer[] = ['q1', 'q2', 'q3', 'q4'];
const WORKERS = 2;
/** Encoded tile bodies kept, so a new date or a new device builds the pages again without a request. */
const BODIES = 96 * 2 ** 20;
const SNOW_TILE = 256 * 256 * 2;

/**
 * The ground of a release: the ruleset, the archives of the ground, colour and snow layers, and the
 * workers that build the ground pages of a material tile on a date.
 */
export class GroundSource {
    requests = 0;
    bytes = 0;
    failed = 0;
    private readonly bodies = new Map<string, Promise<ArrayBuffer | undefined>>();
    private readonly sizes = new Map<string, number>();
    private held = 0;
    private readonly workers = Array.from({ length: WORKERS }, () => new Worker(new URL('./ground-worker.ts', import.meta.url), { type: 'module' }));
    private readonly waiting = new Map<number, { resolve: (pages: (GroundPage | null)[]) => void; reject: (error: Error) => void }>();
    private next = 0;

    private constructor(readonly rules: Rules, private readonly sources: Partial<Record<Layer, Source>>, readonly seasons: number) {
        for (const worker of this.workers) {
            worker.onmessage = ({ data }) => {
                const wait = this.waiting.get(data.id);
                this.waiting.delete(data.id);
                if (data.error) wait?.reject(new Error(data.error));
                else wait?.resolve(data.pages);
            };
        }
    }

    /** The ground of a config; undefined when it has no ruleset or no material archive. */
    static async open(config: PlannerConfig): Promise<GroundSource | undefined> {
        const urls = config.layers as Partial<Record<string, string>>;
        if (!config.trailview_rules || !urls[NAMES.material]) return undefined;
        const response = await releaseFetch(config.trailview_rules, { signal: requestSignal() });
        if (!response.ok) throw new Error(`The TrailView rules answered ${response.status}.`);
        const rules = rulesOf(await response.json());
        const entries = await Promise.all(Object.entries(NAMES).map(async ([layer, name]) => {
            const url = urls[name];
            return [layer, url ? await openSource(url) : undefined] as const;
        }));
        const sources = Object.fromEntries(entries.filter(([, source]) => source));
        const seasons = sources.snow ? snowMeta(sources.snow.metadata).seasons : 0;
        return new GroundSource(rules, sources, seasons);
    }

    /** The finest level with ground pages: one finer than the material's last zoom. */
    get finest(): number {
        return this.sources.material!.maxZoom + 1;
    }

    /** The credits of the layers. */
    get attribution(): string[] {
        return [...new Set(Object.values(this.sources).map(source => String(source.metadata.attribution ?? '')).filter(Boolean))];
    }

    /** The colour variant of the release, from the colour metadata. */
    get colour(): string {
        return String(this.sources.q1?.metadata.variant ?? 'none');
    }

    /** The four pages of material tile z/x/y on a date, with the two quarters around it; null where it has no ground. */
    async pages(z: number, x: number, y: number, quarters: [number, number], date: GroundDate, world: number): Promise<(GroundPage | null)[]> {
        const at = (layer: Layer, zoom: number, planar = false) => this.body(layer, Math.min(zoom, this.sources[layer]?.maxZoom ?? -1), x, y, z, planar);
        const colour = this.sources.q1, zc = Math.min(z, colour?.maxZoom ?? -1);
        const coloured = colour !== undefined && zc >= colour.minZoom;
        const snow = this.sources.snow, zs = Math.min(z + 1, snow?.maxZoom ?? -1);
        const snowBodies = !snow ? [] : zs === z + 1
            ? [0, 1, 2, 3].map(q => this.body('snow', zs, 2 * x + (q & 1), 2 * y + (q >> 1), zs, true))
            : [at('snow', zs, true)];
        const [material, lithology, region, a, b, ndvimax, ...snowTiles] = await Promise.all([
            at('material', z), at('lithology', z), at('region', z),
            ...(coloured ? [at(QUARTERS[quarters[0]], zc), at(QUARTERS[quarters[1]], zc), at('ndvimax', zc)] : [undefined, undefined, undefined]),
            ...snowBodies,
        ]);
        const job: GroundJob = {
            z, x, y, material, lithology, region, quarters: coloured ? [a, b] : undefined, ndvimax, snow: snowTiles, seasons: this.seasons, date, world,
        };
        const id = this.next++;
        return new Promise((resolve, reject) => {
            this.waiting.set(id, { resolve, reject });
            this.workers[id % WORKERS].postMessage({ id, job });
        });
    }

    /** The body of the tile of a layer at a zoom that holds tile z/x/y of zoom `of`; undefined where the layer has none. */
    private async body(layer: Layer, zoom: number, x: number, y: number, of: number, planar = false): Promise<Body | undefined> {
        const source = this.sources[layer];
        if (!source || zoom < source.minZoom || zoom > of) return undefined;
        const tx = x >> (of - zoom), ty = y >> (of - zoom), key = `${layer}/${zoom}/${tx}/${ty}`;
        let entry = this.bodies.get(key);
        if (entry) {
            this.bodies.delete(key);
            this.bodies.set(key, entry);
        } else {
            entry = source.get(zoom, tx, ty, requestSignal()).then(body => {
                this.requests++;
                if (body && planar && body.byteLength % SNOW_TILE) throw new Error(`Snow tile ${key} has ${body.byteLength} bytes.`);
                this.keep(key, body?.byteLength ?? 0);
                return body;
            });
            this.bodies.set(key, entry);
            entry.catch(() => {
                this.failed++;
                this.bodies.delete(key);
            });
        }
        const body = await entry;
        // A copy: the workers keep what they decode, and the bodies stay here for the next date.
        return body && { key, z: zoom, x: tx, y: ty, body: body.slice(0), planar };
    }

    private keep(key: string, bytes: number): void {
        this.bytes += bytes;
        this.sizes.set(key, bytes);
        this.held += bytes;
        for (const old of this.bodies.keys()) {
            if (this.held <= BODIES) break;
            if (old === key) continue;
            this.bodies.delete(old);
            this.held -= this.sizes.get(old) ?? 0;
            this.sizes.delete(old);
        }
    }
}
