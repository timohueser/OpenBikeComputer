import { PAGE } from '../stream/heights';
import { NEAR } from './bins';
import { key, type Node } from './lod';
import type { Page } from './pages';

/** Texels of a page whose cliff share reads a neighbour's heights or moves (cliff.wgsl): two, as whole packed texels of four. */
const REACH = 4;
/** Bytes of `CliffJob` in cliff.wgsl. */
export const CLIFF_JOB_BYTES = 32;

/** The texels from `lo` to `hi` (not included) of a page with a mask that the cliff pass makes. */
export interface CliffJob {
    page: Page;
    lo: [number, number];
    hi: [number, number];
}

/**
 * The cliff pass's jobs of a frame: the whole page of each page with a mask whose mask or heights changed, and the texels
 * of each other page with a mask that face a neighbour of its level whose heights or mask changed; at most one job per
 * page and neighbour. `changed` holds the nodes whose heights in the LOD map changed; `last` holds the mask layer and
 * the node of each page with a mask by name, and takes this frame's.
 */
export function cliffJobs(pages: Map<string, Page>, last: Map<string, [number, Node]>, changed: Node[]): CliffJob[] {
    const masked = new Map([...pages].filter(([, page]) => page.mask !== undefined && page.drawn !== undefined));
    const whole = new Set(changed.map(key)), moved = [...changed];
    for (const [name, [mask, node]] of last) {
        if (masked.get(name)?.mask === mask) continue;
        whole.add(name);
        moved.push(node);
    }
    for (const [name, page] of masked) {
        if (last.has(name)) continue;
        whole.add(name);
        moved.push(page.node);
    }
    last.clear();
    for (const [name, page] of masked) last.set(name, [page.mask!, page.node]);
    const jobs: CliffJob[] = [...masked].filter(([name]) => whole.has(name)).map(([, page]) => ({ page, lo: [0, 0], hi: [PAGE, PAGE] }));
    const seen = new Set<string>();
    for (const { level, x, y } of moved) {
        if (level < NEAR) continue;
        // The neighbour lies at -dx, -dy from the page.
        for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
            const name = key({ level, x: x + dx, y: y + dy }), page = masked.get(name), side = `${name}/${dx}/${dy}`;
            if (!page || whole.has(name) || seen.has(side)) continue;
            seen.add(side);
            jobs.push({ page, lo: [dx < 0 ? PAGE - REACH : 0, dy < 0 ? PAGE - REACH : 0], hi: [dx > 0 ? REACH : PAGE, dy > 0 ? REACH : PAGE] });
        }
    }
    return jobs;
}

/** Jobs as `CliffJob` in cliff.wgsl. */
export function packCliffJobs(jobs: CliffJob[]): ArrayBuffer {
    const data = new ArrayBuffer(Math.max(1, jobs.length) * CLIFF_JOB_BYTES), ints = new Int32Array(data);
    jobs.forEach(({ page: { node, mask }, lo, hi }, k) => ints.set([node.x * PAGE, node.y * PAGE, node.level, mask!, ...lo, ...hi], (CLIFF_JOB_BYTES / 4) * k));
    return data;
}
