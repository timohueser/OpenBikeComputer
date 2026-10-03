import { PMTiles } from 'pmtiles';

/** A data-layer archive; `get` resolves to the decompressed body, or undefined for an absent tile. */
export interface TileArchive {
    metadata: Record<string, unknown>; minZoom: number; maxZoom: number; bounds: [number, number, number, number];
    get(z: number, x: number, y: number, signal?: AbortSignal): Promise<ArrayBuffer | undefined>;
}

/**
 * A hosted region serves TileJSON (with the archive metadata at the top level) and tiles from the
 * tile service, which answers 204 for an absent tile; a local region reads the PMTiles archive.
 */
export async function openTileArchive(url: string, label: string): Promise<TileArchive> {
    if (new URL(url).pathname.endsWith('.json')) {
        const response = await fetch(url);
        if (!response.ok) throw new Error(`${label} TileJSON answered ${response.status}.`);
        const json = await response.json();
        const template: string = json.tiles[0];
        return {
            metadata: json, minZoom: json.minzoom, maxZoom: json.maxzoom, bounds: json.bounds,
            async get(z, x, y, signal) {
                const tile = await fetch(template.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y)), { signal });
                if (tile.status === 204) return undefined;
                if (!tile.ok) throw new Error(`${label} tile ${z}/${x}/${y} answered ${tile.status}.`);
                return tile.arrayBuffer();
            },
        };
    }
    const tiles = new PMTiles(url);
    const [header, metadata] = await Promise.all([tiles.getHeader(), tiles.getMetadata() as Promise<Record<string, unknown>>]);
    return {
        metadata, minZoom: header.minZoom, maxZoom: header.maxZoom, bounds: [header.minLon, header.minLat, header.maxLon, header.maxLat],
        get: async (z, x, y, signal) => (await tiles.getZxy(z, x, y, signal))?.data,
    };
}
