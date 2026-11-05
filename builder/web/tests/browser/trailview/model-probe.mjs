// The viewer's heights.wgsl against the crate's height_at at the points that the debug tool's
// `heights --gpu-check --out DIR` writes: the GPU's height and kind at each point, with Rust's label
// and terrain, and the largest difference. One GPU user at a time:
//   flock /tmp/trailview-gpu.lock node tests/browser/trailview/model-probe.mjs DIR
import { copyFileSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { include } from '../../../src/lib/trailview/gpu/include.ts';
import { launch, serve } from './harness.js';

/** Over this many metres a height differs. */
const TOLERANCE_M = 0.002;
const ROOT = new URL('../../../src/lib/trailview/', import.meta.url).pathname;

const [dir] = process.argv.slice(2);
if (!dir) throw new Error('model-probe.mjs DIR (of debug heights --gpu-check)');
const points = JSON.parse(readFileSync(join(dir, 'points.json'), 'utf8'));
const code = include('terrain/heights.wgsl', path => readFileSync(ROOT + path, 'utf8')) + `
@group(0) @binding(1) var<storage, read> probe_in: array<vec4u>;
@group(0) @binding(2) var<storage, read> probe_terrain: array<f32>;
@group(0) @binding(3) var<storage, read_write> probe_out: array<vec2f>;
@compute @workgroup_size(64) fn probe(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= arrayLength(&probe_out)) { return; }
    let v = probe_in[id.x];
    probe_out[id.x] = height_at(v.x, bitcast<vec2f>(v.yz), v.w, probe_terrain[id.x], 0u);
}`;

const root = mkdtempSync(join(tmpdir(), 'model-probe-'));
writeFileSync(join(root, 'probe.html'), '<!doctype html><title>probe</title>');
copyFileSync(join(dir, 'blocks.bin'), join(root, 'blocks.bin'));
const { origin, close } = await serve(root);
const browser = await launch(true);
const page = await browser.newPage();
await page.goto(`${origin}/probe.html`);
const result = await page.evaluate(async ({ code, points }) => {
    const adapter = await navigator.gpu.requestAdapter();
    const device = await adapter.requestDevice();
    device.pushErrorScope('validation');
    const module = device.createShaderModule({ code });
    const errors = (await module.getCompilationInfo()).messages.filter(m => m.type === 'error');
    if (errors.length) return { errors: errors.map(m => `${m.lineNum}: ${m.message}`) };
    const all = new Uint32Array(await (await fetch('/blocks.bin')).arrayBuffer());
    const rows = Math.ceil(all.length / 4096);
    const padded = new Uint32Array(rows * 4096);
    padded.set(all);
    const texture = device.createTexture({ size: [4096, rows], format: 'r32uint', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
    device.queue.writeTexture({ texture }, padded, { bytesPerRow: 4096 * 4, rowsPerImage: rows }, [4096, rows]);
    const input = new Uint32Array(points.length * 4);
    const floats = new Float32Array(input.buffer);
    const terrain = new Float32Array(points.length);
    points.forEach(([block, x, y, label, t], k) => {
        input[4 * k] = block;
        floats[4 * k + 1] = x;
        floats[4 * k + 2] = y;
        input[4 * k + 3] = label;
        terrain[k] = t;
    });
    const buffer = data => {
        const b = device.createBuffer({ size: data.byteLength, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
        device.queue.writeBuffer(b, 0, data);
        return b;
    };
    const size = points.length * 8;
    const output = device.createBuffer({ size, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC });
    const read = device.createBuffer({ size, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
    const pipeline = await device.createComputePipelineAsync({ layout: 'auto', compute: { module, entryPoint: 'probe' } });
    const group = device.createBindGroup({
        layout: pipeline.getBindGroupLayout(0),
        entries: [
            { binding: 0, resource: texture.createView() },
            { binding: 1, resource: { buffer: buffer(input) } },
            { binding: 2, resource: { buffer: buffer(terrain) } },
            { binding: 3, resource: { buffer: output } },
        ],
    });
    const encoder = device.createCommandEncoder();
    const pass = encoder.beginComputePass();
    pass.setPipeline(pipeline);
    pass.setBindGroup(0, group);
    pass.dispatchWorkgroups(Math.ceil(points.length / 64));
    pass.end();
    encoder.copyBufferToBuffer(output, 0, read, 0, size);
    device.queue.submit([encoder.finish()]);
    await read.mapAsync(GPUMapMode.READ);
    const out = new Float32Array(read.getMappedRange().slice(0));
    const error = await device.popErrorScope();
    return { error: error?.message, out: Array.from(out) };
}, { code, points });
await browser.close();
close();
if (result.errors) {
    console.log(result.errors.slice(0, 10).join('\n'));
    process.exit(1);
}
if (result.error) throw new Error(result.error);
const places = {};
const rows = points.map(([block, x, y, label, t, h, kind, name], k) => {
    const gh = result.out[2 * k], gk = result.out[2 * k + 1];
    const p = (places[name] ??= { points: 0, max_mm: 0, kinds_differ: 0 });
    const d = Math.abs(gh - h);
    p.points++;
    p.max_mm = Math.max(p.max_mm, d * 1000);
    p.kinds_differ += gk !== kind;
    return { block, q: [x, y], label, t, rust: [h, kind], gpu: [gh, gk] };
});
writeFileSync(join(dir, 'probe.json'), JSON.stringify(rows));
const max = Math.max(...Object.values(places).map(p => p.max_mm));
const differ = Object.values(places).reduce((n, p) => n + p.kinds_differ, 0);
for (const [name, p] of Object.entries(places)) console.log(`${name}: ${p.points} points, max ${p.max_mm.toFixed(3)} mm, kinds differ ${p.kinds_differ}`);
console.log(`${rows.length} points: max ${max.toFixed(3)} mm, kinds differ ${differ}`);
process.exit(max <= TOLERANCE_M * 1000 && differ === 0 ? 0 : 1);
