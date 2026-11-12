/** The style config and the MapLibre worker and protocols that each map of the app needs. */
import * as maplibregl from 'maplibre-gl';
import mapWorkerUrl from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url';
import { Protocol } from 'pmtiles';
import { config } from './map-data';
import { releaseProtocol, releaseUrl } from './release';

// MapLibre loads the release objects of the style through `releaseProtocol`.
export const styleConfig = { ...config, basemap: releaseUrl(config.basemap), glyphs: releaseUrl(config.glyphs), sprites: releaseUrl(config.sprites) };

let registered = false;

/** Sets the worker of MapLibre and its protocols `pmtiles` and `release`, once for the page. */
export function useMapProtocols(): void {
    if (registered) return;
    registered = true;
    maplibregl.setWorkerUrl(mapWorkerUrl);
    maplibregl.addProtocol('pmtiles', new Protocol().tile);
    maplibregl.addProtocol('release', releaseProtocol);
}
