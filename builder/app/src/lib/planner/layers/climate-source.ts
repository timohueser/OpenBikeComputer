import type { Coordinate } from '../map-types';
import { OVERVIEW, cellAt, climateMeta, climateTile, locate, type ClimateMeta, type ClimateTile, type Level } from './climate';
import { openTileArchive, type TileArchive } from './tile-archive';

export type TileGetter = (level: Level, x: number, y: number) => Promise<ClimateTile | undefined>;

export interface ClimateSource { meta: ClimateMeta; bounds: [number, number, number, number]; tile: TileGetter }

/**
 * Each tile is requested once and stays decoded: a region archive is a few megabytes, so the map,
 * the route and the point chart share every tile without a cache limit.
 */
export function climateSource(archive: TileArchive): ClimateSource {
    const tiles = new Map<string, Promise<ClimateTile | undefined>>();
    const load = async (level: Level, x: number, y: number) => {
        const body = await archive.get(level, x, y);
        return body && climateTile(level, x, y, new Uint8Array(body));
    };
    return {
        meta: climateMeta(archive.metadata),
        bounds: archive.bounds,
        tile(level, x, y) {
            const key = `${level}/${x}/${y}`;
            let entry = tiles.get(key);
            if (!entry) {
                entry = load(level, x, y);
                tiles.set(key, entry);
                entry.catch(() => tiles.delete(key));
            }
            return entry;
        },
    };
}

const opened = new Map<string, Promise<ClimateSource>>();

/** One source per archive URL, so the climate layers share its tiles; a failed open is retried on the next call. */
export function openClimate(url: string): Promise<ClimateSource> {
    let entry = opened.get(url);
    if (!entry) {
        entry = openTileArchive(url, 'Climate').then(climateSource);
        opened.set(url, entry);
        entry.catch(() => opened.delete(url));
    }
    return entry;
}

/** A cell of a decoded tile. */
export interface CellRef { tile: ClimateTile; index: number }

/**
 * The cell of each coordinate on one level; undefined where the archive has no tile. Each tile the
 * line touches is requested once. The overview level carries everything a route needs, and one
 * overview tile covers four detail tiles, so a route reads the overview and a point chart the detail.
 */
export async function sampleLine(line: Coordinate[], tile: TileGetter, level: Level = OVERVIEW): Promise<(CellRef | undefined)[]> {
    const spots = line.map(coordinate => locate(level, cellAt(coordinate)));
    const keys = [...new Set(spots.map(({ x, y }) => `${x}/${y}`))];
    const loaded = new Map(await Promise.all(keys.map(async key => {
        const [x, y] = key.split('/').map(Number);
        return [key, await tile(level, x, y)] as const;
    })));
    return spots.map(({ x, y, index }) => {
        const found = loaded.get(`${x}/${y}`);
        return found && { tile: found, index };
    });
}
