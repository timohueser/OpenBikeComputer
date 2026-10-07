import { addProtocol, type Map, type RasterLayerSpecification, type RasterTileSource, type RequestParameters } from 'maplibre-gl';
import type { Bounds } from './archive';

/** Where a raster layer has tiles, known once its archive has opened. */
export interface RasterExtent { minzoom?: number; maxzoom: number; bounds?: Bounds; attribution: string }

export interface RasterOptions {
    id: string;
    extent(): Promise<RasterExtent>;
    /** Draws tile z/x/y of a look from the look alone; undefined leaves the tile empty. */
    draw(look: string, z: number, x: number, y: number, signal: AbortSignal): Promise<ImageData | undefined>;
    paint?: RasterLayerSpecification['paint'];
    /** Hears the error of a failed tile, and no error when a new look starts, which draws every tile again. */
    report(error?: unknown): void;
}

/** An empty body draws a transparent tile. */
const empty = () => ({ data: new ArrayBuffer(0) });

/**
 * A data layer of raster tiles under the relief, so the hillshade shades them. The look, everything
 * a tile depends on such as the date and the theme, is in the tile URL: a new look replaces the URL,
 * MapLibre redraws each tile, and the old tiles stay drawn until the new ones replace them. The layer
 * is decoration, so a failed tile is empty and reports to the layer instead of failing the map.
 */
export class Raster {
    private look = '';
    private generation = new AbortController();
    private readonly protocol: string;

    constructor(private options: RasterOptions) {
        this.protocol = `obc-${options.id}`;
    }

    sync(map: Map, shown: boolean, look: string) {
        const { id } = this.options;
        if (!shown) {
            // A hidden layer starts no work; it redraws its tiles when it shows again.
            this.generation.abort();
            this.look = '';
            if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', 'none');
            return;
        }
        const changed = look !== this.look;
        if (changed) {
            this.generation.abort();
            this.generation = new AbortController();
            this.look = look;
            this.options.report();
        }
        const template = `${this.protocol}://${look}/{z}/{x}/{y}`;
        const source = map.getSource<RasterTileSource>(id);
        if (source) {
            map.setLayoutProperty(id, 'visibility', 'visible');
            if (changed || source.tiles?.[0] !== template) source.setTiles([template]);
            return;
        }
        addProtocol(this.protocol, this.tile);
        const generation = this.generation.signal;
        this.options.extent().then(extent => {
            // A newer look installs instead; a theme change, which loads a new style, has a newer look.
            if (generation.aborted || map.getSource(id)) return;
            map.addSource(id, { type: 'raster', tiles: [template], tileSize: 256, ...extent });
            map.addLayer({ id, type: 'raster', source: id, paint: { 'raster-fade-duration': 0, ...this.options.paint } }, map.getLayer('relief') ? 'relief' : undefined);
        }, () => {});
    }

    /**
     * A tile of an older look draws the current look: an error would hide it until its reload ends,
     * and the tiles blink while the date moves. A newer look cancels the work for the older one.
     */
    private tile = async ({ url }: RequestParameters, abort: AbortController) => {
        const [z, x, y] = url.split('/').slice(-3).map(Number);
        while (this.look) {
            const look = this.look, generation = this.generation.signal, signal = AbortSignal.any([abort.signal, generation]);
            try {
                const image = await this.options.draw(look, z, x, y, signal);
                signal.throwIfAborted();
                return image ? { data: await createImageBitmap(image) } : empty();
            } catch (error) {
                if (abort.signal.aborted) throw error;
                if (generation.aborted) continue;
                this.options.report(error);
                return empty();
            }
        }
        throw new DOMException('Hidden layer', 'AbortError');
    };
}
