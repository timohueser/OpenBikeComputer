import contour from 'maplibre-contour';
import type * as maplibre from 'maplibre-gl';
import { clientConfig } from './client-config';

/** The plugin's worker messages give up after 20 s, so a longer fetch limit has no effect. */
const DEM_TIMEOUT_MS = 20_000;
const TERRAIN_SOURCES = new Set(['terrain', 'contours']);
const FIRST_RETRY_MS = 5_000;
const RETRIES = 3;

type TileError = maplibre.ErrorEvent & { sourceId?: string; tile?: { tileID: { canonical: { x: number; y: number; z: number } } } };

/** Keep one bounded DEM cache per map surface; the plugin has no manager disposal API. */
export function terrainSource(url: string) {
    let source: InstanceType<typeof contour.DemSource> | undefined;
    let users = 0;
    return {
        acquire(library: typeof maplibre) {
            if (clientConfig.terrainWorkerUrl) contour.workerUrl = new URL(clientConfig.terrainWorkerUrl, window.location.href).href;
            source ??= new contour.DemSource({ url, maxzoom: 12, worker: true, cacheSize: 64, encoding: 'terrarium', timeoutMs: DEM_TIMEOUT_MS });
            if (users++ === 0) source.setupMaplibre(library);
            let active = true;
            const dem = source;
            return { dem, release() {
                if (!active) return;
                active = false;
                if (--users === 0) {
                    library.removeProtocol(dem.sharedDemProtocolId);
                    library.removeProtocol(dem.contourProtocolId);
                }
            } };
        },
    };
}

/**
 * Relief and contours are decoration, so a terrain error is never a map failure. A failed terrain
 * tile loads again a few times, each wait twice the last, because a slow link usually recovers.
 * The returned check is true when an error belongs to a terrain source.
 */
export function terrainRetry(map: maplibre.Map) {
    const attempts = new Map<string, number>();
    const timers = new Set<ReturnType<typeof setTimeout>>();
    map.once('remove', () => timers.forEach(clearTimeout));
    return (event: TileError) => {
        const source = event.sourceId;
        if (!source || !TERRAIN_SOURCES.has(source)) return false;
        if (!event.tile) return true;
        const { x, y, z } = event.tile.tileID.canonical;
        const key = `${source}/${z}/${x}/${y}`;
        const attempt = attempts.get(key) ?? 0;
        if (attempt === RETRIES) return true;
        attempts.set(key, attempt + 1);
        const timer = setTimeout(() => {
            timers.delete(timer);
            if (map.getSource(source)) map.refreshTiles(source, [{ x, y, z }]);
        }, FIRST_RETRY_MS * 2 ** attempt);
        timers.add(timer);
        return true;
    };
}
