// Spike 2 (never merges, `pages=mesh`): the level-18 and level-17 pages under the computed pieces drawn from the
// pieces' own triangles, against the pages that the stamp pass wrote. A debug read after waitForIdle(); nothing
// reaches the frame.
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { key, type Node } from '../terrain/lod';
import type { CellMesh } from './mesh';

const PAGE = 256;
const CELL_UNITS = 1 << 14;

export interface MeshPiece {
    node: Node;
    mesh: CellMesh;
    data: GPUBuffer;
    indices: GPUBuffer;
}

export interface MeshPagesInput {
    gpu: Gpu;
    surface: GPUBindGroup;
    model: GPUTexture;
    heights: GPUTexture;
    scales: GPUBuffer;
    pieces: MeshPiece[];
    /** The layer of a node's drawn page, if it is ready. */
    drawn: (node: Node) => number | undefined;
}

/** One page: the stamped heights, the lookup's height, label, rule word and plain terrain, the raster's heights and label. */
export interface MeshPage {
    name: string;
    cell: string;
    level: number;
    x: number;
    y: number;
    stamped: Float32Array;
    lookH: Float32Array;
    lookLabel: Uint32Array;
    lookWords: Uint32Array;
    plain: Float32Array;
    rasterH: Float32Array;
    rasterB: Float32Array;
    rasterLabel: Uint32Array;
    covered: Uint8Array;
    /** Texels that the step slots alone cover. */
    slotTexels: number;
    lookupMs: number;
    rasterMs: number;
    slotsMs: number;
}

interface Made {
    lookup: GPUComputePipeline;
    raster: GPURenderPipeline;
    slots: GPURenderPipeline;
    computeLayout: GPUBindGroupLayout;
    renderLayout: GPUBindGroupLayout;
}

let made: Made | undefined;

async function pipelines(gpu: Gpu): Promise<Made> {
    const { device } = gpu, { surface } = gpu.once(layouts);
    const C = GPUShaderStage.COMPUTE, V = GPUShaderStage.VERTEX, F = GPUShaderStage.FRAGMENT;
    const computeLayout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: C, buffer: { type: 'read-only-storage' } }, { binding: 2, visibility: C, buffer: { type: 'uniform' } },
        { binding: 3, visibility: C, buffer: { type: 'storage' } }, { binding: 8, visibility: C, texture: { sampleType: 'uint' } },
    ] });
    const renderLayout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: V | F, buffer: { type: 'read-only-storage' } }, { binding: 2, visibility: V | F, buffer: { type: 'uniform' } },
    ] });
    const module = device.createShaderModule({ label: 'mesh pages', code: wgsl('plan/mesh-pages.wgsl') });
    const info = await module.getCompilationInfo();
    const errors = info.messages.filter(m => m.type === 'error');
    if (errors.length) throw new Error(`mesh pages: ${errors.map(m => `${m.lineNum}: ${m.message}`).join('; ')}`);
    const render = (entryPoint: string) => device.createRenderPipelineAsync({
        label: `mesh pages ${entryPoint}`, layout: device.createPipelineLayout({ bindGroupLayouts: [renderLayout, surface] }),
        vertex: { module, entryPoint }, fragment: { module, entryPoint: 'raster_fragment', targets: [{ format: 'rgba32uint' }] },
        primitive: { topology: 'triangle-list', cullMode: 'none' },
    });
    const [lookup, raster, slots] = await Promise.all([
        device.createComputePipelineAsync({ label: 'mesh pages lookup', layout: device.createPipelineLayout({ bindGroupLayouts: [computeLayout, surface] }), compute: { module, entryPoint: 'lookup' } }),
        render('raster_vertex'), render('slot_raster'),
    ]);
    return { lookup, raster, slots, computeLayout, renderLayout };
}

const origin = (slot: number): [number, number, number] => [PAGE * (slot & 7), PAGE * ((slot >> 3) & 7), slot >> 6];

/** The pages under each piece: its 16 level-18 nodes and its 4 level-17 nodes that have a drawn page. */
export async function meshPages(input: MeshPagesInput, only?: Set<string>): Promise<MeshPage[]> {
    const { gpu, pieces } = input, { device } = gpu;
    made ??= await pipelines(gpu);
    const m = made;
    const times = device.features.has('timestamp-query') ? device.createQuerySet({ type: 'timestamp', count: 6 }) : undefined;
    const out: MeshPage[] = [];
    for (const piece of pieces) {
        const cell = piece.node, n = piece.mesh.corners.length / 2;
        const nodes: Node[] = [];
        for (const level of [18, 17]) {
            const k = 1 << (level - 16);
            for (let j = 0; j < k; j++) for (let i = 0; i < k; i++) nodes.push({ level, x: cell.x * k + i, y: cell.y * k + j });
        }
        for (const node of nodes) {
            const name = key(node);
            if (only && !only.has(name)) continue;
            const slot = input.drawn(node);
            if (slot === undefined) continue;
            const shift = 22 - node.level;
            const job = new Int32Array(8);
            job.set([cell.x * CELL_UNITS, cell.y * CELL_UNITS, node.x * PAGE, node.y * PAGE]);
            new Uint32Array(job.buffer).set([node.level, 2 * n, 6 * n, 0], 4);
            void shift;
            const params = device.createBuffer({ size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
            device.queue.writeBuffer(params, 0, job);
            const looked = device.createBuffer({ size: 16 * PAGE * PAGE, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC });
            const target = (label: string) => device.createTexture({ label, size: [PAGE, PAGE], format: 'rgba32uint', usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_SRC });
            const raster = target('mesh pages raster'), slots = target('mesh pages slots');
            const compute = device.createBindGroup({ layout: m.computeLayout, entries: [
                { binding: 0, resource: { buffer: piece.data } }, { binding: 2, resource: { buffer: params } },
                { binding: 3, resource: { buffer: looked } }, { binding: 8, resource: input.model.createView() },
            ] });
            const draw = device.createBindGroup({ layout: m.renderLayout, entries: [{ binding: 0, resource: { buffer: piece.data } }, { binding: 2, resource: { buffer: params } }] });
            const encoder = device.createCommandEncoder();
            const writes = (i: number) => (times ? { querySet: times, beginningOfPassWriteIndex: 2 * i, endOfPassWriteIndex: 2 * i + 1 } : undefined);
            const lookup = encoder.beginComputePass({ label: 'mesh pages lookup', timestampWrites: writes(0) });
            lookup.setPipeline(m.lookup);
            lookup.setBindGroup(0, compute);
            lookup.setBindGroup(1, input.surface);
            lookup.dispatchWorkgroups(PAGE / 8, PAGE / 8);
            lookup.end();
            const pass = (view: GPUTexture, i: number) => encoder.beginRenderPass({
                label: 'mesh pages raster', timestampWrites: writes(i),
                colorAttachments: [{ view: view.createView(), loadOp: 'clear', storeOp: 'store', clearValue: [0, 0, 0, 0] }],
            });
            const tri = pass(raster, 1);
            tri.setPipeline(m.raster);
            tri.setBindGroup(0, draw);
            tri.setBindGroup(1, input.surface);
            tri.setIndexBuffer(piece.indices, 'uint16');
            // Over a level-17 node the strip's level-18 runs do not draw, as the pieces at mesh level 17.
            const runs = piece.mesh.runs.length - (node.level < 18 ? piece.mesh.level18Runs : 0);
            let first = 0;
            piece.mesh.runs.slice(0, runs).forEach(({ count, base }) => {
                tri.drawIndexed(3 * count, 1, first, base);
                first += 3 * count;
            });
            tri.end();
            const sl = pass(slots, 2);
            sl.setPipeline(m.slots);
            sl.setBindGroup(0, draw);
            sl.setBindGroup(1, input.surface);
            if (piece.mesh.steps.length) sl.draw(6 * (piece.mesh.steps.length / 4));
            sl.end();
            const bytes = { heights: PAGE * 2 * PAGE, scale: 256, looked: 16 * PAGE * PAGE, raster: 16 * PAGE * PAGE, slots: 16 * PAGE * PAGE, times: 256 };
            const staging = device.createBuffer({ size: Object.values(bytes).reduce((a, b) => a + b, 0), usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
            let at = 0;
            const offsets: Record<string, number> = {};
            for (const [k, b] of Object.entries(bytes)) {
                offsets[k] = at;
                at += b;
            }
            encoder.copyTextureToBuffer({ texture: input.heights, origin: origin(slot) }, { buffer: staging, offset: offsets.heights, bytesPerRow: PAGE * 2 }, [PAGE, PAGE, 1]);
            encoder.copyBufferToBuffer(input.scales, 8 * slot, staging, offsets.scale, 8);
            encoder.copyBufferToBuffer(looked, 0, staging, offsets.looked, bytes.looked);
            encoder.copyTextureToBuffer({ texture: raster }, { buffer: staging, offset: offsets.raster, bytesPerRow: PAGE * 16 }, [PAGE, PAGE, 1]);
            encoder.copyTextureToBuffer({ texture: slots }, { buffer: staging, offset: offsets.slots, bytesPerRow: PAGE * 16 }, [PAGE, PAGE, 1]);
            let resolved: GPUBuffer | undefined;
            if (times) {
                resolved = device.createBuffer({ size: 48, usage: GPUBufferUsage.QUERY_RESOLVE | GPUBufferUsage.COPY_SRC });
                encoder.resolveQuerySet(times, 0, 6, resolved, 0);
                encoder.copyBufferToBuffer(resolved, 0, staging, offsets.times, 48);
            }
            device.queue.submit([encoder.finish()]);
            await staging.mapAsync(GPUMapMode.READ);
            const all = staging.getMappedRange();
            const codes = new Uint16Array(all, offsets.heights, PAGE * PAGE), [offset, scale] = new Float32Array(all, offsets.scale, 2);
            const stamped = new Float32Array(PAGE * PAGE);
            for (let t = 0; t < stamped.length; t++) stamped[t] = codes[t] === 0xffff ? NaN : offset + codes[t] * scale;
            const lw = new Uint32Array(all, offsets.looked, 4 * PAGE * PAGE), lf = new Float32Array(all, offsets.looked, 4 * PAGE * PAGE);
            const rw = new Uint32Array(all, offsets.raster, 4 * PAGE * PAGE), rf = new Float32Array(all, offsets.raster, 4 * PAGE * PAGE);
            const sw = new Uint32Array(all, offsets.slots, 4 * PAGE * PAGE);
            const page: MeshPage = {
                name, cell: key(cell), level: node.level, x: node.x, y: node.y, stamped,
                lookH: new Float32Array(PAGE * PAGE), lookLabel: new Uint32Array(PAGE * PAGE), lookWords: new Uint32Array(PAGE * PAGE), plain: new Float32Array(PAGE * PAGE),
                rasterH: new Float32Array(PAGE * PAGE), rasterB: new Float32Array(PAGE * PAGE), rasterLabel: new Uint32Array(PAGE * PAGE), covered: new Uint8Array(PAGE * PAGE),
                slotTexels: 0, lookupMs: NaN, rasterMs: NaN, slotsMs: NaN,
            };
            for (let t = 0; t < PAGE * PAGE; t++) {
                page.lookH[t] = lf[4 * t];
                page.lookLabel[t] = lw[4 * t + 1];
                page.lookWords[t] = lw[4 * t + 2];
                page.plain[t] = lf[4 * t + 3];
                page.covered[t] = rw[4 * t + 3];
                page.rasterH[t] = rf[4 * t];
                page.rasterB[t] = rf[4 * t + 1];
                page.rasterLabel[t] = rw[4 * t + 2];
                page.slotTexels += sw[4 * t + 3];
            }
            if (times) {
                const ts = new BigInt64Array(all, offsets.times, 6);
                [page.lookupMs, page.rasterMs, page.slotsMs] = [0, 1, 2].map(i => Number(ts[2 * i + 1] - ts[2 * i]) / 1e6);
            }
            staging.unmap();
            for (const b of [staging, params, looked, resolved]) b?.destroy();
            raster.destroy();
            slots.destroy();
            out.push(page);
        }
    }
    times?.destroy();
    return out;
}

/** Log bins of |Δ| in metres, from 1e-6 m to 1e3 m. */
const BINS = 3600;
const LOG_MIN = -6;
const LOG_SPAN = 9;
const bin = (d: number) => Math.max(0, Math.min(BINS - 1, Math.floor(((Math.log10(Math.max(d, 1e-9)) - LOG_MIN) / LOG_SPAN) * BINS)));
const binValue = (b: number) => 10 ** (LOG_MIN + ((b + 0.5) / BINS) * LOG_SPAN);

export const SPLITS = ['all', 'stamped', 'plan', 'paved', 'slope', 'terrain', 'paved_inside', 'paved_edge', 'slope_inside', 'slope_edge', 'terrain_inside', 'terrain_edge', 'inside', 'edge'] as const;

/** A distribution of |Δ|: exact percentiles of a page, a histogram for totals. */
export class Dist {
    n = 0;
    max = 0;
    labelDiff = 0;
    hist = new Float64Array(BINS);
    values: number[] = [];
    add(d: number, labelDiff: boolean, keep: boolean) {
        const a = Math.abs(d);
        this.n++;
        this.max = Math.max(this.max, a);
        this.hist[bin(a)]++;
        if (labelDiff) this.labelDiff++;
        if (keep) this.values.push(a);
    }
    merge(o: Dist) {
        this.n += o.n;
        this.max = Math.max(this.max, o.max);
        this.labelDiff += o.labelDiff;
        for (let b = 0; b < BINS; b++) this.hist[b] += o.hist[b];
    }
    /** Exact percentiles of the kept values, else from the histogram. */
    summary() {
        if (!this.n) return { n: 0 };
        let q: (p: number) => number;
        if (this.values.length === this.n) {
            const v = Float64Array.from(this.values).sort();
            q = p => v[Math.round((v.length - 1) * p)];
        } else {
            q = p => {
                const want = p * this.n;
                let sum = 0;
                for (let b = 0; b < BINS; b++) if ((sum += this.hist[b]) >= want) return binValue(b);
                return this.max;
            };
        }
        return { n: this.n, p50: q(0.5), p95: q(0.95), p99: q(0.99), max: this.max, label_diff: this.labelDiff / this.n };
    }
}

export type Dists = Record<(typeof SPLITS)[number], Dist>;
export const dists = (): Dists => Object.fromEntries(SPLITS.map(s => [s, new Dist()])) as Dists;

/** The page's difference statistics over the texels that the mesh covers, and the counts of the known limits. */
export function pageStats(page: MeshPage, keep = true) {
    const a = dists(), b = dists(), look = new Dist(), surfaces = new Array(16).fill(0), rules = [0, 0, 0, 0, 0];
    let valid = 0, covered = 0, noHeight = 0, shaped = 0, shapedOutside = 0, slopeOutside = 0, slopeTexels = 0;
    const L = page.lookLabel;
    for (let y = 0; y < PAGE; y++) for (let x = 0; x < PAGE; x++) {
        const t = y * PAGE + x, words = page.lookWords[t], rule = words & 0xff, surface = (words >> 8) & 0xff;
        const s = page.stamped[t], lh = page.lookH[t];
        if (Number.isFinite(s) && lh > -1e29) look.add(lh - s, false, false);
        // Shaped by the plan: the lookup's height off the plain terrain, or a paved label.
        const isShaped = rule >= 2 || (lh > -1e29 && page.plain[t] > -1e29 && Math.abs(lh - page.plain[t]) > 0.01);
        if (isShaped) shaped++;
        if (rule === 1) slopeTexels++;
        if (!page.covered[t]) {
            if (isShaped) shapedOutside++;
            if (rule === 1) slopeOutside++;
            continue;
        }
        covered++;
        if (page.covered[t] !== 1) {
            noHeight++;
            continue;
        }
        if (!Number.isFinite(s) || !(lh > -1e29)) continue;
        valid++;
        rules[Math.min(rule, 4)]++;
        surfaces[surface & 15]++;
        let edge = false;
        for (let dy = -1; dy <= 1 && !edge; dy++) for (let dx = -1; dx <= 1; dx++) {
            const xx = x + dx, yy = y + dy;
            if (xx >= 0 && yy >= 0 && xx < PAGE && yy < PAGE && L[yy * PAGE + xx] !== L[t]) {
                edge = true;
                break;
            }
        }
        const kind = rule >= 2 ? 'paved' : rule === 1 ? 'slope' : 'terrain';
        const where = edge ? 'edge' : 'inside';
        const differs = page.rasterLabel[t] !== L[t];
        // Water and structures moved the stamped page here (the lookup has no stamp): their own split.
        const stampedHere = Math.abs(lh - s) > 0.02;
        const splits = stampedHere ? ['all', 'stamped'] : ['all', 'plan', kind, `${kind}_${where}`, where];
        for (const [d, set] of [[page.rasterH[t] - s, a], [page.rasterB[t] - s, b]] as const) {
            for (const split of splits) set[split as keyof Dists].add(d, differs, keep);
        }
    }
    return { a, b, look, valid, covered, noHeight, shaped, shapedOutside, slopeOutside, slopeTexels, rules, surfaces };
}

/** The per-page summaries and the totals (from the histograms) of a view's mesh pages. */
export function report(pages: MeshPage[]) {
    const total = { a: dists(), b: dists(), look: new Dist() };
    const counts = { valid: 0, covered: 0, noHeight: 0, shaped: 0, shapedOutside: 0, slopeOutside: 0, slopeTexels: 0, slotTexels: 0, pages: pages.length };
    const rows = pages.map(page => {
        const s = pageStats(page);
        for (const k of SPLITS) {
            total.a[k].merge(s.a[k]);
            total.b[k].merge(s.b[k]);
        }
        total.look.merge(s.look);
        for (const k of ['valid', 'covered', 'noHeight', 'shaped', 'shapedOutside', 'slopeOutside', 'slopeTexels'] as const) counts[k] += s[k];
        counts.slotTexels += page.slotTexels;
        return {
            name: page.name, cell: page.cell, level: page.level, valid: s.valid, covered: s.covered, noHeight: s.noHeight, shaped: s.shaped, shapedOutside: s.shapedOutside,
            slopeOutside: s.slopeOutside, slopeTexels: s.slopeTexels, rules: s.rules, surfaces: s.surfaces, slotTexels: page.slotTexels,
            lookupMs: page.lookupMs, rasterMs: page.rasterMs, slotsMs: page.slotsMs,
            a: Object.fromEntries(SPLITS.map(k => [k, s.a[k].summary()])), b: Object.fromEntries(SPLITS.map(k => [k, s.b[k].summary()])), look: s.look.summary(),
        };
    });
    return { counts, rows, total: { a: Object.fromEntries(SPLITS.map(k => [k, total.a[k].summary()])), b: Object.fromEntries(SPLITS.map(k => [k, total.b[k].summary()])), look: total.look.summary() } };
}

/** A page's arrays in one buffer as base64: stamped, lookH, lookLabel, lookWords, plain, rasterH, rasterB, rasterLabel (4 bytes a texel each), covered (1 byte). */
export function dump(page: MeshPage): string {
    const parts = [page.stamped, page.lookH, page.lookLabel, page.lookWords, page.plain, page.rasterH, page.rasterB, page.rasterLabel, page.covered];
    const bytes = new Uint8Array(parts.reduce((n, p) => n + p.byteLength, 0));
    let at = 0;
    for (const p of parts) {
        bytes.set(new Uint8Array(p.buffer, p.byteOffset, p.byteLength), at);
        at += p.byteLength;
    }
    let text = '';
    for (let k = 0; k < bytes.length; k += 0x8000) text += String.fromCharCode(...bytes.subarray(k, k + 0x8000));
    return btoa(text);
}

/** The corners of the computed pieces whose height is neither NONE nor within ±1e5 m: cell, index, point, label, terrain flag, height. */
export async function badCorners(gpu: Gpu, pieces: MeshPiece[]) {
    const { device } = gpu, out: unknown[] = [];
    for (const piece of pieces) {
        const n = piece.mesh.corners.length / 2, bytes = 4 * 6 * n;
        const staging = device.createBuffer({ size: bytes, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
        const encoder = device.createCommandEncoder();
        encoder.copyBufferToBuffer(piece.data, 0, staging, 0, bytes);
        device.queue.submit([encoder.finish()]);
        await staging.mapAsync(GPUMapMode.READ);
        const w = new Uint32Array(staging.getMappedRange()), f = new Float32Array(w.buffer);
        let none = 0;
        for (let k = 0; k < n; k++) {
            const h = f[2 * n + 4 * k], w0 = w[2 * k], w1 = w[2 * k + 1];
            if (h <= -1e29) none++;
            else if (!(Math.abs(h) < 1e5)) out.push({ cell: key(piece.node), k, q: [w0 & 0x7ffff, w1 & 0x7ffff], label: (w0 >>> 19) | (((w1 >>> 19) & 0xfff) << 13), terrain: w1 >>> 31, h });
        }
        out.push({ cell: key(piece.node), corners: n, none });
        staging.unmap();
        staging.destroy();
    }
    return out;
}
