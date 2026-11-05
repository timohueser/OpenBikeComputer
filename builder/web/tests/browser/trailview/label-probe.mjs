// The viewer's model.wgsl against the crate's `find` at the whole-unit points that the debug tool's
// `heights --gpu-check --out DIR` writes to labels.json: the label of each point, on and beside the
// candidates' edges too. Prints the points whose labels differ. One GPU user at a time:
//   flock /tmp/trailview-gpu.lock node tests/browser/trailview/label-probe.mjs DIR
import { copyFileSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { include } from '../../../src/lib/trailview/gpu/include.ts';
import { launch, serve } from './harness.js';

const ROOT = new URL('../../../src/lib/trailview/', import.meta.url).pathname;
const [dir] = process.argv.slice(2);
if (!dir) throw new Error('label-probe.mjs DIR (of debug heights --gpu-check)');
const points = JSON.parse(readFileSync(join(dir, 'labels.json'), 'utf8'));
const code = include('terrain/model.wgsl', path => readFileSync(ROOT + path, 'utf8')) + `
@group(0) @binding(1) var<storage, read> probe_in: array<vec4u>;
@group(0) @binding(3) var<storage, read_write> probe_out: array<u32>;
@compute @workgroup_size(64) fn probe(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= arrayLength(&probe_out)) { return; }
    let v = probe_in[id.x];
    probe_out[id.x] = model_point(v.x, bitcast<vec2i>(v.yz), 0.0).label;
}`;

const root = mkdtempSync(join(tmpdir(), 'label-probe-'));
writeFileSync(join(root, 'probe.html'), '<!doctype html><title>probe</title>');
copyFileSync(join(dir, 'blocks.bin'), join(root, 'blocks.bin'));
const { origin, close } = await serve(root);
const browser = await launch(true);
const page = await browser.newPage();
await page.goto(`${origin}/probe.html`);
const result = await page.evaluate(async ({ code, points }) => {
    const device = await (await navigator.gpu.requestAdapter()).requestDevice();
    const module = device.createShaderModule({ code });
    const errors = (await module.getCompilationInfo()).messages.filter(m => m.type === 'error');
    if (errors.length) return { errors: errors.map(m => `${m.lineNum}: ${m.message}`) };
    const all = new Uint32Array(await (await fetch('/blocks.bin')).arrayBuffer());
    const rows = Math.ceil(all.length / 4096), padded = new Uint32Array(rows * 4096);
    padded.set(all);
    const texture = device.createTexture({ size: [4096, rows], format: 'r32uint', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
    device.queue.writeTexture({ texture }, padded, { bytesPerRow: 4096 * 4, rowsPerImage: rows }, [4096, rows]);
    const input = new Int32Array(points.length * 4);
    points.forEach(([block, x, y], k) => input.set([block, x, y, 0], 4 * k));
    const inBuffer = device.createBuffer({ size: input.byteLength, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
    device.queue.writeBuffer(inBuffer, 0, input);
    const size = points.length * 4;
    const output = device.createBuffer({ size, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC });
    const read = device.createBuffer({ size, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
    const pipeline = await device.createComputePipelineAsync({ layout: 'auto', compute: { module, entryPoint: 'probe' } });
    const group = device.createBindGroup({ layout: pipeline.getBindGroupLayout(0), entries: [
        { binding: 8, resource: texture.createView() }, { binding: 1, resource: { buffer: inBuffer } }, { binding: 3, resource: { buffer: output } },
    ] });
    const encoder = device.createCommandEncoder(), pass = encoder.beginComputePass();
    pass.setPipeline(pipeline);
    pass.setBindGroup(0, group);
    pass.dispatchWorkgroups(Math.ceil(points.length / 64));
    pass.end();
    encoder.copyBufferToBuffer(output, 0, read, 0, size);
    device.queue.submit([encoder.finish()]);
    await read.mapAsync(GPUMapMode.READ);
    return { out: Array.from(new Uint32Array(read.getMappedRange().slice(0))) };
}, { code, points });
await browser.close();
close();
if (result.errors) {
    console.log(result.errors.slice(0, 10).join('\n'));
    process.exit(1);
}
const differ = points.flatMap(([block, x, y, label], k) => (result.out[k] === label ? [] : [{ block, q: [x, y], rust: label, gpu: result.out[k] }]));
for (const row of differ.slice(0, 20)) console.log(JSON.stringify(row));
console.log(`${points.length} points: ${differ.length} labels differ`);
process.exit(differ.length ? 1 : 0);
