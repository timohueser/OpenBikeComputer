import { DETAIL, OVERVIEW, climateTile, type Level } from '../../src/lib/planner/layers/climate';

// The plane tables of specs/planner-climate-tiles.md: name, indexes, bytes per value.
const SPEC: Record<Level, { cells: number; planes: [string, number, number][] }> = {
    [OVERVIEW]: { cells: 384, planes: [['orography', 1, 2], ['lapse_tmax', 12, 1], ['lapse_tmin', 12, 1], ['rose', 192, 1], ['wet_share', 52, 1],
        ['rain', 52, 1], ['tmax', 52, 1], ['tmin', 52, 1], ['wind', 52, 1]] },
    [DETAIL]: { cells: 96, planes: [['orography', 1, 2], ['lapse_tmax', 12, 1], ['lapse_tmin', 12, 1], ['wet_days', 520, 1], ['rain', 520, 1],
        ['tmax', 520, 1], ['tmin', 520, 1], ['wind', 520, 1]] },
};
const SIGNED = new Set(['orography', 'lapse_tmax', 'lapse_tmin', 'tmax', 'tmin']);

/** A tile body with every value missing, and a writer of raw codes. */
export function specTile(level: Level, x = 0, y = 0) {
    const { cells, planes } = SPEC[level];
    const starts = new Map<string, number>();
    let size = 0;
    for (const [name, count, bytes] of planes) { starts.set(name, size); size += count * cells * bytes; }
    const body = new Uint8Array(size);
    const view = new DataView(body.buffer);
    const set = (name: string, index: number, cell: number, code: number) => {
        const [, , bytes] = planes.find(p => p[0] === name)!;
        const at = starts.get(name)! + (index * cells + cell) * bytes;
        if (bytes === 2) view.setInt16(at, code, true);
        else if (SIGNED.has(name)) view.setInt8(at, code);
        else view.setUint8(at, code);
    };
    for (const [name, count] of planes) for (let i = 0; i < count; i++) for (let c = 0; c < cells; c++) set(name, i, c, name === 'orography' ? -32768 : SIGNED.has(name) ? -128 : 255);
    return { body, set, tile: () => climateTile(level, x, y, body) };
}
