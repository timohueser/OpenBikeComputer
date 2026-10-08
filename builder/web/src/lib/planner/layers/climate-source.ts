import { SvelteMap } from 'svelte/reactivity';
import type { Coordinate } from '../map-types';
import { archiveOf, eachTile, openArchive, type TileArchive, type Decoding, type Source } from './archive';
import { DETAIL, OVERVIEW, cellAt, climateMeta, climateTile, locate, type ClimateMeta, type ClimateTile, type Level } from './climate';

export type ClimateSource = TileArchive<ClimateMeta, ClimateTile>;
export type TileGetter = (level: Level, x: number, y: number) => Promise<ClimateTile | null>;

/** A region archive is a few megabytes, so the map, the route and the point chart keep every tile they read. */
const CLIMATE: Decoding<ClimateMeta, ClimateTile> = {
    meta: climateMeta,
    decode: (body, _meta, z, x, y) => climateTile(z as Level, x, y, body),
    bytes: tile => tile.body.byteLength,
    budgetBytes: 32 * 2 ** 20,
};

export const climateSource = (source: Source): ClimateSource => archiveOf(source, CLIMATE);

/** One source per archive URL, so the climate layers share its tiles. */
export const openClimate = (url: string): Promise<ClimateSource> => openArchive(url, CLIMATE);

/** A cell of a decoded tile. */
export interface CellRef { tile: ClimateTile; index: number }

/** Arrivals of the tiles that `detailCell` asked for; reading a key makes a reactive reader run again on arrival. */
const arrivals = new SvelteMap<string, number>();

/**
 * The detail cell of a coordinate for a point chart, read in reactive code: undefined while its tile
 * loads, and the read starts the load of a missing tile. Null where the archive has no tile.
 */
export function detailCell(source: ClimateSource, coordinate: Coordinate): CellRef | null | undefined {
    const { x, y, index } = locate(DETAIL, cellAt(coordinate)), key = `${x}/${y}`;
    arrivals.get(key);
    const tile = source.loaded(DETAIL, x, y);
    if (tile === undefined) source.tile(DETAIL, x, y).then(() => arrivals.set(key, (arrivals.get(key) ?? 0) + 1), () => {});
    return tile && { tile, index };
}

/**
 * The cell of each coordinate on one level; undefined where the archive has no tile. Each tile the
 * line touches is requested once. The overview level carries everything a route needs, and one
 * overview tile covers four detail tiles, so a route reads the overview and a point chart reads
 * the detail of its one point with `detailCell`.
 */
export async function sampleLine(line: Coordinate[], tile: TileGetter, level: Level = OVERVIEW): Promise<(CellRef | undefined)[]> {
    const spots = line.map(coordinate => locate(level, cellAt(coordinate)));
    const cells = new Array<CellRef | undefined>(line.length).fill(undefined);
    await eachTile(spots, (x, y) => tile(level, x, y), (found, indexes) => {
        if (found) for (const i of indexes) cells[i] = { tile: found, index: spots[i].index };
    });
    return cells;
}
