// PROTOTYPE (#3651, never merges): the placements of the tunnel parts from the bake (parts.json), found by the
// OSM end of a tunnel way. No imports: bins.ts and the structures read it.

export interface Fit {
    open: number; collar: number; head: number; wl: number; wr: number; tops: number[]; L: number[]; R: number[]; floor: number; F: number[]; grade: number; foot: number; cfoot: number; LB: number[]; RB: number[]; S0: number | null;
}
export interface BoxEnd { node: number; x: number; y: number; ux: number; uy: number; floor: number; head: number; len: number; L: number[]; R: number[]; F: number[]; LB: number[]; RB: number[]; foot?: number; sfoot?: number }
export interface Placement {
    node: number; x: number; y: number; ux: number; uy: number; g: string; look: string; w: number; h: number; round: boolean;
    floor0: number; shift: number; kind: string; rung: string;
    v1?: Fit; v2?: Fit; twin?: { node: number; side: number; gap: number; stagger: number } | null;
    free?: { len: number; foot: number }; hb?: number; ends?: BoxEnd[]; drawer?: number;
}
const byNode = new Map<number, Placement>();
/** The placement that draws a skipped end's part: a wider way in the same tube. */
export const drawerOf = (p: Placement): Placement => (p.kind === 'skip' && p.drawer !== undefined ? byNode.get(p.drawer) ?? p : p);

let variant: 'v1' | 'v2' = 'v1';
const cells = new Map<string, Placement[]>();
const CELL = 4096;
export let partsOn = false;

export function setParts(which: string, list: Placement[]): void {
    variant = which === 'v2' ? 'v2' : 'v1';
    partsOn = true;
    for (const p of list) {
        byNode.set(p.node, p);
        const key = `${Math.floor(p.x / CELL)},${Math.floor(p.y / CELL)}`;
        (cells.get(key) ?? cells.set(key, []).get(key)!).push(p);
    }
}

/** The placement whose OSM end lies within 1.5 m of a world point. */
export function partAt(x: number, y: number): Placement | undefined {
    if (!partsOn) return undefined;
    const tol = 1.5 / 0.0254;
    let best: Placement | undefined, d = tol;
    for (let i = -1; i <= 1; i++) for (let j = -1; j <= 1; j++) {
        for (const p of cells.get(`${Math.floor(x / CELL) + i},${Math.floor(y / CELL) + j}`) ?? []) {
            const e = Math.hypot(p.x - x, p.y - y);
            if (e < d) [best, d] = [p, e];
        }
    }
    return best;
}

/** The placements whose OSM end lies in a box of world units. */
export function partsIn(x0: number, y0: number, x1: number, y1: number): Placement[] {
    if (!partsOn) return [];
    const out: Placement[] = [];
    for (let i = Math.floor(x0 / CELL); i <= Math.floor(x1 / CELL); i++) for (let j = Math.floor(y0 / CELL); j <= Math.floor(y1 / CELL); j++) {
        for (const p of cells.get(`${i},${j}`) ?? []) if (p.x >= x0 && p.x < x1 && p.y >= y0 && p.y < y1) out.push(p);
    }
    return out;
}

/** The fit of the variant in use. */
export const fitOf = (p: Placement): Fit | undefined => p[variant] ?? p.v1;

/**
 * Metres from the OSM end that a part takes along its tunnel line: there the tunnel stamps no ground (bins.ts). An
 * underpass takes its whole line; an end that a twin or the other end draws takes as much as the drawing part.
 */
export function partReach(x: number, y: number): number {
    const found = partAt(x, y), p = found && drawerOf(found);
    if (!p || p.kind === 'none') return 0;
    if (p.kind === 'box') return 1e4;
    if (p.kind === 'skip') return p.rung.startsWith('underpass') ? 1e4 : 0;
    if (p.kind === 'free') return p.free!.len;
    const f = fitOf(p)!;
    return f.open + 0.8 + Math.max(f.collar, 1) + 1;
}
