// Adapters between the v2 cell model and the things a map component draws
// (#1038). Everything here is arithmetic over `outline.ts` / `grid.ts` results —
// no Leaflet, no DOM — so the decisions a drawing embodies (which band an
// outline is drawn from, what counts as "one patch") are testable without a
// browser.

import { parseCellId, type CellId, type UBox } from "../catalog/v2/grid";
import type { CatalogV2 } from "../catalog/v2/manifest";
import { coverageRings, type RingPoint } from "../catalog/v2/outline";

/**
 * The band whose cells *are* "the selection's coverage" on screen — the finest
 * detail band.
 *
 * §8 U1 draws "the actual coverage outline of the current selection", and the
 * selection covers ground precisely at the fine bands and generously at the
 * coarse one (the covering context cells ride along silently, §8's coarse-band
 * decision — drawing them would show a country-sized halo nobody chose). The
 * network band shares the fine band's cell size by design, so the finest
 * geometry band is the honest outline for everything the rider will actually
 * see drawn on glass.
 */
export function detailBandId(catalog: CatalogV2): string {
    const geometry = catalog.schema.bands.filter((b) => b.lods.length > 0 && b.role !== "coarse");
    const candidates = geometry.length ? geometry : catalog.schema.bands;
    return candidates.reduce((a, b) => (b.cell_log2 < a.cell_log2 ? b : a)).id;
}

/** Canonical id strings → parsed cells, the shape `outline.ts` takes. */
export function parseCells(ids: readonly string[]): CellId[] {
    return ids.map(parseCellId);
}

/** Signed shoelace area of a ring in (lon, lat) axes: positive for the
 *  counter-clockwise winding `coverageRings` gives outer rings. */
function signedArea(ring: RingPoint[]): number {
    let sum = 0;
    for (let k = 1; k < ring.length; k++) {
        const [aLat, aLon] = ring[k - 1];
        const [bLat, bLon] = ring[k];
        sum += aLon * bLat - bLon * aLat;
    }
    return sum / 2;
}

/**
 * How many disjoint patches a cell set forms — the number the corridor panel
 * turns into "1 gap between routes" (patches − 1). Outer rings wind
 * counter-clockwise and holes clockwise (`coverageRings`' contract), so patches
 * are exactly the positive-area rings.
 */
export function patchCount(cells: Iterable<CellId>): number {
    return coverageRings(cells).filter((ring) => signedArea(ring) > 0).length;
}

/** µdeg ring → degree `[lat, lon]` pairs, the order Leaflet's polygons take. */
export function ringToDegrees(ring: RingPoint[]): [number, number][] {
    return ring.map(([lat, lon]) => [lat / 1e6, lon / 1e6]);
}

/** µdeg box → degree `[[south, west], [north, east]]` corner pair. */
export function uboxToDegrees(box: UBox): [[number, number], [number, number]] {
    return [
        [box.minLat / 1e6, box.minLon / 1e6],
        [box.maxLat / 1e6, box.maxLon / 1e6],
    ];
}

/** Degree bounds → a µdeg box, rounded outward so the box never shrinks past
 *  what was drawn. */
export function degreesToUbox(south: number, west: number, north: number, east: number): UBox {
    return {
        minLat: Math.floor(south * 1e6),
        minLon: Math.floor(west * 1e6),
        maxLat: Math.ceil(north * 1e6),
        maxLon: Math.ceil(east * 1e6),
    };
}
