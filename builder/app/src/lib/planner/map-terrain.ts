import contour from 'maplibre-contour';
import type * as maplibre from 'maplibre-gl';
import { clientConfig } from './client-config';

/** The plugin's worker messages give up after 20 s, so a longer fetch limit has no effect. */
const DEM_TIMEOUT_MS = 20_000;
const TERRAIN_SOURCES = new Set(['terrain', 'contours']);
const FIRST_RETRY_MS = 5_000;
const RETRIES = 3;

type TileEvent = { sourceId?: string; tile?: { tileID: { canonical: { x: number; y: number; z: number } } } };
type Retry = { source: string; key: string; x: number; y: number; z: number };

function terrainTile({ sourceId: source, tile }: TileEvent): Retry | undefined {
    if (!source || !TERRAIN_SOURCES.has(source) || !tile) return;
    const { x, y, z } = tile.tileID.canonical;
    return { source, key: `${source}/${z}/${x}/${y}`, x, y, z };
}

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
 * Only one retry runs at a time: tiles that failed together would otherwise split the link again.
 * The returned check is true when an error belongs to a terrain source.
 */
export function terrainRetry(map: maplibre.Map) {
    const attempts = new Map<string, number>();
    const due: Retry[] = [];
    const timers = new Set<ReturnType<typeof setTimeout>>();
    let running: Retry | undefined;
    const later = (ms: number, action: () => void) => {
        const timer = setTimeout(() => { timers.delete(timer); action(); }, ms);
        timers.add(timer);
    };
    const next = () => {
        if (running) return;
        running = due.shift();
        if (!running) return;
        const retry = running;
        // A tile that loaded since it failed needs no retry.
        if (!attempts.has(retry.key) || !map.getSource(retry.source)) return settle(retry.key);
        map.refreshTiles(retry.source, [retry]);
        // A tile that left the view neither loads nor fails; every request ends within the DEM limit.
        later(DEM_TIMEOUT_MS, () => { if (running === retry) settle(retry.key); });
    };
    const settle = (key: string) => {
        if (running?.key !== key) return;
        running = undefined;
        next();
    };
    map.on('sourcedata', (event) => {
        const tile = terrainTile(event as TileEvent);
        if (!tile) return;
        attempts.delete(tile.key);
        settle(tile.key);
    });
    map.once('remove', () => timers.forEach(clearTimeout));
    return (event: maplibre.ErrorEvent & TileEvent) => {
        if (!event.sourceId || !TERRAIN_SOURCES.has(event.sourceId)) return false;
        const tile = terrainTile(event);
        if (!tile) return true;
        settle(tile.key);
        const attempt = attempts.get(tile.key) ?? 0;
        if (attempt === RETRIES) return true;
        attempts.set(tile.key, attempt + 1);
        later(FIRST_RETRY_MS * 2 ** attempt, () => { due.push(tile); next(); });
        return true;
    };
}
