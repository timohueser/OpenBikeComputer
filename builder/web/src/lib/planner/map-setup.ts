/** The style config and the MapLibre worker and protocols that each map of the app needs. */
import * as maplibregl from 'maplibre-gl';
import mapWorkerUrl from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url';
import { Protocol } from 'pmtiles';
import { config } from './map-data';
import { terrainSource } from './map-terrain';
import { releaseProtocol, releaseUrl } from './release';

// MapLibre loads the release objects of the style through `releaseProtocol`.
export const styleConfig = { ...config, basemap: releaseUrl(config.basemap), glyphs: releaseUrl(config.glyphs), sprites: releaseUrl(config.sprites) };

/** The credits of a view of the map: the basemap (OSM data, its land cover, Protomaps), the terrain and `layers`. */
export const creditsWith = (layers: string[]) =>
    [...new Set([config.attribution, config.landcover_attribution ?? '', 'Protomaps', config.terrain_attribution, ...layers].filter(Boolean))];

/** The one terrain source of the app: each map with the relief shares its cache of terrain tiles. */
export const terrain = terrainSource(config.terrain);

/** MapLibre's credits control, whose compact form starts closed and opens only at its button. */
export class ClosedAttribution extends maplibregl.AttributionControl {
    constructor(options: maplibregl.AttributionControlOptions) {
        super(options);
        // MapLibre opens a compact control when it turns compact, which is when its text arrives or the map gets narrow.
        const update = this._updateCompact;
        this._updateCompact = () => {
            const shown = this._container.classList.contains('maplibregl-compact-show');
            update();
            if (!shown) this._updateCompactMinimize();
        };
    }
}

let registered = false;

/** Sets the worker of MapLibre and its protocols `pmtiles` and `release`, once for the page. */
export function useMapProtocols(): void {
    if (registered) return;
    registered = true;
    maplibregl.setWorkerUrl(mapWorkerUrl);
    maplibregl.addProtocol('pmtiles', new Protocol().tile);
    maplibregl.addProtocol('release', releaseProtocol);
}
