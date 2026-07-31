// Adapters between the v2 cell model and the things a map component draws
// (#1038). Everything here is arithmetic over `outline.ts` / `grid.ts` results —
// no Leaflet, no DOM — so the decisions a drawing embodies (which band an
// outline is drawn from, what counts as "one patch") are testable without a
// browser.

import { parseCellId, type CellId, type UBox } from "../catalog/v2/grid";
import type { CatalogV2 } from "../catalog/v2/manifest";
import { coverageRings, mergeCellRects, type RingPoint } from "../catalog/v2/outline";

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

/**
 * {@link mergeCellRects} over a set that may span cell sizes — the hole set
 * does, now that holes are drawn from every band (#1041 A5), and the merge
 * itself takes cells of one size. One merge per size; the rectangles of
 * different sizes may overlap on the ground, which for a hatch is simply the
 * same warning twice in the same place.
 */
export function mergeMixedCellRects(ids: readonly string[]): UBox[] {
    const bySize = new Map<number, CellId[]>();
    for (const cell of parseCells(ids)) {
        const group = bySize.get(cell.log2);
        if (group) group.push(cell);
        else bySize.set(cell.log2, [cell]);
    }
    return [...bySize.values()].flatMap((cells) => mergeCellRects(cells));
}

/**
 * The cells of `candidates` that touch a cell of `holes` — edge or corner —
 * on the same-size lattice.
 *
 * This is #1041 A9's decided hatch rule for partial detail cells: a partial
 * cell is only *news* where it abuts a hole, because there the detail visibly
 * stops and bare backdrop begins. A partial cell along the outline with
 * nothing missing beside it is border-overhang normality (#1025 measured most
 * fine-band border cells partial for every real extract), and hatching it
 * would re-impose exactly the noise tax §8 U1 rejected. Corner adjacency
 * counts: a diagonal staircase step reads as "next to the hole" on screen.
 *
 * Both arguments are detail-band cells, so one size: adjacency is judged on
 * the candidates' own lattice, and ids of any other size in `holes` are
 * ignored rather than approximated across lattices.
 */
export function cellsTouchingHoles(candidates: readonly string[], holes: readonly string[]): string[] {
    if (candidates.length === 0 || holes.length === 0) return [];
    const stride = 2 ** 19; // unique for every in-world index, exact in a double
    const log2 = parseCellId(candidates[0]).log2;
    const holeKeys = new Set<number>();
    for (const id of holes) {
        const cell = parseCellId(id);
        if (cell.log2 === log2) holeKeys.add(cell.i * stride + cell.j);
    }
    if (holeKeys.size === 0) return [];
    return candidates.filter((id) => {
        const { i, j } = parseCellId(id);
        for (let di = -1; di <= 1; di++) {
            for (let dj = -1; dj <= 1; dj++) {
                if ((di || dj) && holeKeys.has((i + di) * stride + (j + dj))) return true;
            }
        }
        return false;
    });
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
