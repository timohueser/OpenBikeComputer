/** Finds the parts of the rider's routes that leave the area of a device map. It needs the wasm core of the map builder. */
import type { RegionEntry } from '../../lib/catalog/manifest';
import { cellIndexHas } from '../../lib/catalog/satellites';
import { cellContaining, formatCellId, type UBox } from '../../lib/core/grid';
import { degreesToUbox, detailBandId } from '../../lib/coverage/shape';
import type { CoverageStore } from '../../lib/coverage/store.svelte';
import { distanceM } from '../../lib/library/summary';

/** The room in degrees between a route and the edge of the box around it, about 1 km: the map then holds the ground
 * beside the line. */
const ROOM = 0.01;

export interface Outside {
    id: string;
    /** The length of the route outside the area. */
    metres: number;
    /** A box around each part of the route outside the area. */
    boxes: UBox[];
    /** The smallest region at the middle of them that is not in the area, or null. */
    region: RegionEntry | null;
}

/**
 * The routes that leave the selection of `store`, each with what brings it into the area. `line` is the summary line of
 * a route: longitude, latitude, … A point is outside when the catalog has map data there and the selection does not
 * hold it; ground with no map data cannot be added, so it does not count. A piece of the line between a point inside
 * and a point outside counts with half its length. A route with no length outside, such as a route of one point, is
 * not in the result.
 */
export function routesOutside(store: CoverageStore, routes: { id: string; line: number[] }[]): Outside[] {
    const band = store.catalog.schema.bands.find(entry => entry.id === detailBandId(store.catalog));
    const index = band && store.indices?.get(band.id);
    if (!band || !index) return [];
    const inside = new Set(store.resolution?.cellsByBand.get(band.id));
    const outside = (lon: number, lat: number) => {
        const id = formatCellId(cellContaining(band.cell_log2, Math.round(lat * 1e6), Math.round(lon * 1e6)));
        return !inside.has(id) && cellIndexHas(index, id);
    };
    return routes.flatMap(({ id, line }) => {
        const out = Array.from({ length: line.length / 2 }, (_, i) => outside(line[2 * i], line[2 * i + 1]));
        let metres = 0;
        for (let i = 1; i < out.length; i++) {
            const share = (Number(out[i - 1]) + Number(out[i])) / 2;
            if (share) metres += share * distanceM(line[2 * i - 2], line[2 * i - 1], line[2 * i], line[2 * i + 1]);
        }
        if (metres < 0.5) return [];
        // A run holds the points outside the area and the point before and after them, so its box holds the whole part.
        const runs: number[][] = [];
        out.forEach((is, i) => {
            if (!is && !out[i - 1] && !out[i + 1]) return;
            const run = runs.at(-1);
            if (run?.at(-1) === i - 1) run.push(i);
            else runs.push([i]);
        });
        const boxes = runs.map(run => {
            const lons = run.map(i => line[2 * i]), lats = run.map(i => line[2 * i + 1]);
            return degreesToUbox(Math.min(...lats) - ROOM, Math.min(...lons) - ROOM, Math.max(...lats) + ROOM, Math.max(...lons) + ROOM);
        });
        const beyond = out.flatMap((is, i) => (is ? [i] : [])), middle = beyond[beyond.length >> 1];
        const region = store.regionsAt(Math.round(line[2 * middle + 1] * 1e6), Math.round(line[2 * middle] * 1e6))
            .find(entry => !store.hasRegion(entry.id)) ?? null;
        return [{ id, metres, boxes, region }];
    });
}
