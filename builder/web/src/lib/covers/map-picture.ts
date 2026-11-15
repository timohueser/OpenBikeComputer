/** Draws a still picture of the planner map around the line of a route, with no labels, for a card with no cover. */
import * as maplibregl from 'maplibre-gl';
import type { CoverPoints, Summary } from '../library/records';
import { creditsWith, styleConfig, useMapProtocols } from '../planner/map-setup';
import { mapStyle } from '../planner/map-style';
import type { StillFailure } from '../trailview/still';
import { pack } from './line';

const PICTURE_WIDTH = 768, PICTURE_HEIGHT = 512;
/** The line fits inside the middle 720 × 300 pixels, the band that `crop` shows in each box from 3:2 to 2.2:1. */
const PADDING = { top: 106, bottom: 106, left: 24, right: 24 };
/** A short loop keeps the tiles of the basemap. */
const MAX_ZOOM = 13;
/** Chosen, not measured: well above the time of a picture of a long route on a slow link. */
const TIMEOUT_MS = 20_000;
const QUALITY = 0.85;
/** The layers of the planner map that are no part of a picture, besides each layer of text and icons. */
const LEFT_OUT = new Set(['contour-lines', 'planner-pois']);

export interface MapPictureResult { image: Blob; points: CoverPoints; credits: string[] }
export type MapPictureMaker = (summary: Summary, dark: boolean, signal: AbortSignal) => Promise<MapPictureResult | 'none'>;

const failure = (name: StillFailure, message: string) => Object.assign(new Error(message), { name });

/** The picture of the map at its zoom, or `none` when the line is outside the map data. It fails with the names of `still`. */
export const mapPicture: MapPictureMaker = async (summary, dark, signal) => {
    const [west, south, east, north] = summary.bounds, data = styleConfig.bounds;
    if (west > data[2] || east < data[0] || south > data[3] || north < data[1]) return 'none';
    useMapProtocols();
    const container = document.createElement('div');
    container.style.cssText = `position: fixed; top: 0; left: 0; width: ${PICTURE_WIDTH}px; height: ${PICTURE_HEIGHT}px; visibility: hidden; pointer-events: none`;
    document.body.append(container);
    const done = new AbortController();
    let map: maplibregl.Map | undefined;
    try {
        return await new Promise<MapPictureResult>((resolve, reject) => {
            const fail = (name: StillFailure, message: string) => reject(failure(name, message));
            const timer = setTimeout(() => fail('timeout', `The map picture took more than ${TIMEOUT_MS} ms.`), TIMEOUT_MS);
            done.signal.addEventListener('abort', () => clearTimeout(timer));
            // A hidden page draws no frame.
            const hidden = () => document.hidden && fail('hidden', 'The page became hidden.');
            const aborted = () => signal.aborted && fail('aborted', 'The map picture was aborted.');
            hidden();
            aborted();
            if (document.hidden || signal.aborted) return;
            document.addEventListener('visibilitychange', hidden, { signal: done.signal });
            signal.addEventListener('abort', aborted, { signal: done.signal });
            map = new maplibregl.Map({
                container, style: style(dark), bounds: [west, south, east, north], fitBoundsOptions: { padding: PADDING },
                pixelRatio: 1, canvasContextAttributes: { preserveDrawingBuffer: true }, interactive: false, fadeDuration: 0,
                maxZoom: MAX_ZOOM, attributionControl: false, renderWorldCopies: false,
            });
            map.on('error', event => !('name' in event.error && event.error.name === 'AbortError') && fail('tiles-failed', `A tile of the map picture failed: ${event.error?.message}`));
            map.once('idle', () => {
                const n = summary.line.length / 2, x = new Float32Array(n), y = new Float32Array(n);
                for (let i = 0; i < n; i++) ({ x: x[i], y: y[i] } = map!.project([summary.line[2 * i], summary.line[2 * i + 1]]));
                // One distance gives the line one width, as on a cover seen from above.
                const points = pack({ x, y, distance: new Float32Array(n).fill(1000), visible: new Uint8Array(n).fill(1) });
                encode(map!.getCanvas()).then(image => resolve({ image, points, credits: creditsWith([]) }), reject);
            });
        });
    } finally {
        done.abort();
        // Releases the WebGL context.
        map?.remove();
        container.remove();
    }
};

/** The planner map in the theme, with no text, icons, contour lines or places. */
function style(dark: boolean): maplibregl.StyleSpecification {
    const full = mapStyle(dark ? 'dark' : 'light', styleConfig, styleConfig.terrain, '');
    const { basemap, terrain } = full.sources;
    return { version: 8, sources: { basemap, terrain }, layers: full.layers.filter(layer => layer.type !== 'symbol' && !LEFT_OUT.has(layer.id)) };
}

async function encode(canvas: HTMLCanvasElement): Promise<Blob> {
    const blob = (type: string) => new Promise<Blob>((resolve, reject) => canvas.toBlob(made => made ? resolve(made) : reject(new Error('The map picture has no image.')), type, QUALITY));
    // A browser that cannot encode a type gives PNG.
    const webp = await blob('image/webp');
    return webp.type === 'image/webp' ? webp : blob('image/jpeg');
}
