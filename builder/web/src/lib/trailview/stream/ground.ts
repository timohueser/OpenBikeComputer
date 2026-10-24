import type { PlannerConfig } from '../../planner/config';
import { decodedTiles, openSource, requestSignal, type Source, type Tiles } from '../../planner/layers/archive';
import { seasonDay } from '../../planner/layers/snow';
import { releaseFetch } from '../../planner/release';
import type { GroundResult } from '../ground/pages';
import { rulesOf, type Rules } from '../rules/regions';
import type { GroundDate } from './ground-pages';
import type { Body, GroundJob } from './ground-worker';

/** The archives of the release (specs/trailview-tiles.md), by their names in the catalogue's `layers`. */
const NAMES = {
    material: 'trailview-material', lithology: 'trailview-lithology', region: 'trailview-region', ndvimax: 'trailview-colour-ndvimax',
    q1: 'trailview-colour-q1', q2: 'trailview-colour-q2', q3: 'trailview-colour-q3', q4: 'trailview-colour-q4',
} as const;
type Layer = keyof typeof NAMES;
const QUARTERS: Layer[] = ['q1', 'q2', 'q3', 'q4'];
/** The snow day archives (specs/trailview-tiles.md#snow-days): band b holds the day steps 4b to 4b + 3. */
const SNOW_BANDS = 46, BAND_STEPS = 4, SNOW_DAY = 256 * 256;
const WORKERS = 2;
/** Tile bodies kept per archive, so a new device or a page that comes back builds again without a request. */
const BODIES = 16 * 2 ** 20;

/** The snow day band of a day step. */
export const snowBand = (step: number) => Math.min(SNOW_BANDS - 1, Math.floor(step / BAND_STEPS));
const snowName = (band: number) => `trailview-snow-${String(band).padStart(2, '0')}`;
/** The source of the snow day archive of a band; undefined where the release has none. */
const snowSource = (urls: Partial<Record<string, string>>, band: number) => {
    const url = urls[snowName(band)];
    return url ? openSource(url) : Promise.resolve(undefined);
};
const leftOut = (name: string) => (error: unknown) => void console.warn(`TrailView leaves out ${name}: ${error instanceof Error ? error.message : error}`);

/** An open archive and the bodies of its tiles. */
interface Archive { source: Source; tiles: Tiles<ArrayBuffer> }

/**
 * The ground of a release: the ruleset, the archives of the ground, colour and snow layers, and the
 * workers that build the ground pages of a material tile on a date.
 */
export class GroundSource {
    requests = 0;
    bytes = 0;
    failed = 0;
    private readonly archives: Partial<Record<Layer, Archive>>;
    /** The snow day archive of one band, opened on first use: a date in another band opens its own. */
    private snow?: { band: number; archive: Promise<Archive | undefined> };
    private snowCredit = '';
    private readonly workers = Array.from({ length: WORKERS }, () => new Worker(new URL('./ground-worker.ts', import.meta.url), { type: 'module' }));
    private readonly waiting = new Map<number, { worker: Worker; resolve: (data: GroundResult) => void; reject: (error: Error) => void }>();
    private next = 0;

    private constructor(readonly rules: Rules, sources: Partial<Record<Layer, Source>>, private readonly urls: Partial<Record<string, string>>) {
        this.archives = Object.fromEntries(Object.entries(sources).map(([layer, source]) => [layer, { source, tiles: this.kept(source, body => body.buffer) }]));
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
     * The ground of a config, with the snow of `date` open; undefined when it has no ruleset or no
     * material archive. Only the rules and the material fail it: another archive that does not open is
     * left out.
     */
    static async open(config: PlannerConfig, date: string): Promise<GroundSource | undefined> {
        const urls = config.layers as Partial<Record<string, string>>;
        if (!config.trailview_rules || !urls[NAMES.material]) return undefined;
        const response = await releaseFetch(config.trailview_rules, { signal: requestSignal() });
        if (!response.ok) throw new Error(`The TrailView rules answered ${response.status}.`);
        const rules = rulesOf(await response.json());
        const band = snowBand(seasonDay(date).index);
        const [entries, snow] = await Promise.all([
            Promise.all(Object.entries(NAMES).map(async ([layer, name]) => {
                const url = urls[name];
                if (!url) return [layer, undefined] as const;
                const opened = openSource(url);
                return [layer, await (layer === 'material' ? opened : opened.catch(leftOut(name)))] as const;
            })),
            snowSource(urls, band).catch(leftOut(snowName(band))),
        ]);
        const ground = new GroundSource(rules, Object.fromEntries(entries.filter(([, source]) => source)), urls);
        // A band that did not open opens again on the first load.
        if (snow) ground.snow = { band, archive: Promise.resolve(ground.snowArchive(snow)) };
        return ground;
    }

    /** The finest level with ground pages: one finer than the material's last zoom. */
    get finest(): number {
        return this.archives.material!.source.maxZoom + 1;
    }

    /** The credits of the layers. */
    get attribution(): string[] {
        const credits = Object.values(this.archives).map(({ source }) => String(source.metadata.attribution ?? ''));
        return [...new Set([...credits, this.snowCredit].filter(Boolean))];
    }

    /** The coarsest level whose pages carry the satellite colour. */
    get coloured(): number {
        return (this.archives.q1?.source.minZoom ?? Infinity) + 1;
    }

    /** The colour variant of the release, from the colour metadata. */
    get colour(): string {
        return String(this.archives.q1?.source.metadata.variant ?? 'none');
    }

    /**
     * The four pages of material tile z/x/y on a date, with the two quarters around it; null where it
     * has no ground. A failed material tile fails them; a failed tile of another layer leaves that
     * layer out, and the pages are not complete. The snow is the band of the date's day step. Without
     * `fine`, pages whose four snow tiles one zoom finer have not arrived take the one snow tile of zoom
     * z, and are `coarse`.
     */
    async pages(z: number, x: number, y: number, quarters: [number, number], date: GroundDate, world: number, fine = false): Promise<GroundResult> {
        let complete = true;
        const optional = (body: Promise<Body | undefined>) => body.catch(() => void (complete = false));
        const at = (layer: Layer, zoom: number) => {
            const archive = this.archives[layer], body = this.body(layer, archive, Math.min(zoom, archive?.source.maxZoom ?? -1), x, y, z);
            return layer === 'material' ? body : optional(body);
        };
        const colour = this.archives.q1?.source, zc = Math.min(z, colour?.maxZoom ?? -1);
        const coloured = colour !== undefined && zc >= colour.minZoom;
        const days = await this.snowDays(snowBand(date.snowIndex)).catch(() => void (complete = false)), zs = z + 1;
        const finer = days && zs <= days.source.maxZoom ? [0, 1, 2, 3].map(q => [2 * x + (q & 1), 2 * y + (q >> 1)]) : [];
        const coarse = finer.length > 0 && !fine && finer.some(([sx, sy]) => days!.tiles.loaded(zs, sx, sy) === undefined);
        const snowBodies = !days ? [] : finer.length && !coarse
            ? finer.map(([sx, sy]) => optional(this.body('snow', days, zs, sx, sy, zs, true)))
            : [optional(this.body('snow', days, Math.min(z, days.source.maxZoom), x, y, z, true))];
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
        return { pages: built.pages, complete: complete && built.complete, coarse };
    }

    /** The body of the tile of an archive at a zoom that holds tile z/x/y of zoom `of`; undefined where the archive has none. */
    private async body(name: string, archive: Archive | undefined, zoom: number, x: number, y: number, of: number, planar = false): Promise<Body | undefined> {
        if (!archive || zoom < archive.source.minZoom || zoom > of) return undefined;
        const tx = x >> (of - zoom), ty = y >> (of - zoom), body = await archive.tiles.tile(zoom, tx, ty);
        // A copy for the worker to take: the bodies stay here.
        return body ? { key: `${name}/${zoom}/${tx}/${ty}`, z: zoom, x: tx, y: ty, body: body.slice(0), planar } : undefined;
    }

    /**
     * The snow day archive of a band; undefined where the release has none. A band that does not open
     * fails the loads that wait for it, and the next load opens it again.
     */
    private snowDays(band: number): Promise<Archive | undefined> {
        if (this.snow?.band !== band) {
            const archive = snowSource(this.urls, band).then(source => source && this.snowArchive(source));
            archive.catch(error => {
                leftOut(snowName(band))(error);
                if (this.snow?.archive === archive) this.snow = undefined;
            });
            this.snow = { band, archive };
        }
        return this.snow.archive;
    }

    private snowArchive(source: Source): Archive {
        this.snowCredit = String(source.metadata.attribution ?? '');
        return {
            source, tiles: this.kept(source, body => {
                if (body.byteLength !== SNOW_DAY) throw new Error(`A snow day tile has ${body.byteLength} bytes.`);
                return body.buffer;
            }),
        };
    }

    /** The bodies of the tiles of an archive, counted in the requests, the bytes and the failures. */
    private kept(source: Source, decode: (body: Uint8Array<ArrayBuffer>) => ArrayBuffer): Tiles<ArrayBuffer> {
        return decodedTiles(async (z, x, y, signal) => {
            try {
                const body = await source.get(z, x, y, signal);
                this.requests++;
                this.bytes += body?.byteLength ?? 0;
                return body;
            } catch (error) {
                this.failed++;
                throw error;
            }
        }, decode, BODIES, body => body.byteLength);
    }
}
