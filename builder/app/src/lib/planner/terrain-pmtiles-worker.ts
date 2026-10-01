import { PMTiles } from 'pmtiles';

const request = globalThis.fetch.bind(globalThis);
const archives = new Map<string, PMTiles>();

// Adapt tile transport only; the pinned contour worker keeps its decoder and algorithms.
globalThis.fetch = async (input, init) => {
    const url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url);
    const match = url.pathname.match(/^\/(maps|map-cutout)\/terrain\/(\d+)\/(\d+)\/(\d+)\.webp$/);
    if (!match) return request(input, init);
    const archiveURL = new URL(`/${match[1]}/terrain.pmtiles`, url).href;
    let archive = archives.get(archiveURL);
    if (!archive) { archive = new PMTiles(archiveURL); archives.set(archiveURL, archive); }
    const tile = await archive.getZxy(+match[2], +match[3], +match[4], init?.signal ?? undefined);
    if (!tile) throw new Error(`Missing terrain tile ${url.pathname}`);
    return new Response(tile.data, { headers: { 'Content-Type': 'image/webp' } });
};
