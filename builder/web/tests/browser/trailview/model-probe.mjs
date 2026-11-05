// GPU probe of the one road model: model_at and height_at of the viewer's WGSL at given world points,
// read back. node model-probe.mjs OUT.json lat,lon,half_m,step_texels ...
import { readFileSync, writeFileSync } from 'node:fs';
import { launch, serve } from './harness.js';
const ROOT = new URL('../../../src/lib/trailview/', import.meta.url).pathname;
const swap = { 'terrain/model-words.wgsl': null };
function compose(path) {
    const seen = new Set();
    const go = file => {
        if (seen.has(file)) return '';
        seen.add(file);
        if (file in swap) return '@group(0) @binding(1) var model_words: texture_2d<u32>;\n';
        const folder = file.slice(0, file.lastIndexOf('/') + 1);
        return readFileSync(ROOT + file, 'utf8').replace(/^\/\/ #include (\S+)$/gm, (_, n) => go(norm(folder + n)));
    };
    return go(path);
}
function norm(p) { const out = []; for (const x of p.split('/')) { if (x === '..') out.pop(); else out.push(x); } return out.join('/'); }
const code = compose('terrain/heights.wgsl') + `
@group(0) @binding(0) var<storage, read_write> io: array<vec4f>;
@compute @workgroup_size(64) fn probe(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if (i >= arrayLength(&io) / 2u) { return; }
    let v = io[2u * i];
    let w = vec2i(bitcast<i32>(v.x), bitcast<i32>(v.y));
    let mp = model_at(w, vec2f(0.0), 0.0, 1e9);
    var h = vec2f(-1e9, -1.0);
    if (mp.block != 0u) { h = height_at(mp.block, mp.q, mp.label, v.z); }
    io[2u * i + 1u] = vec4f(bitcast<f32>(mp.label), h.x, h.y, bitcast<f32>(mp.block));
}`;
const [out, ...places] = process.argv.slice(2);
const UNITS = 2 ** 30;
const pts = [];
for (const place of places) {
    const [lat, lon, half, step] = place.split(',').map(Number);
    const x = (lon + 180) / 360 * UNITS, y = (1 - Math.asinh(Math.tan(lat * Math.PI / 180)) / Math.PI) / 2 * UNITS;
    const mpu = 2 * Math.PI * 6371000 / UNITS * Math.cos(lat * Math.PI / 180);
    const n = Math.round(half / (16 * mpu * step));
    for (let j = -n; j <= n; j++) for (let i = -n; i <= n; i++) {
        const wx = Math.floor(x / 16 + i * step) * 16 + 8, wy = Math.floor(y / 16 + j * step) * 16 + 8;
        pts.push([wx, wy, 1000 + 0.37 * (pts.length % 13) - 2.0 * ((i + j) & 3)]);
    }
}
const model = readFileSync(process.env.OBC_MODEL ?? '/media/timo/OSM1/tmp/ground-plan/one-model/records/model.bin');
const { origin, close } = await serve();
const browser = await launch(true);
const page = await browser.newPage();
page.on('console', m => console.log('console', m.text().slice(0, 300)));
await page.goto(`${origin}/trailview.html?lat=0&lon=0`).catch(() => {});
const result = await page.evaluate(async ({ code, pts, model, mprobe }) => {
    const adapter = await navigator.gpu.requestAdapter();
    const device = await adapter.requestDevice();
    device.pushErrorScope('validation');
    const module = device.createShaderModule({ code });
    const info = await module.getCompilationInfo();
    const errs = info.messages.filter(m => m.type === 'error').map(m => `${m.lineNum}: ${m.message}`);
    if (errs.length) return { errors: errs };
    const words = new Uint32Array(Uint8Array.from(atob(model), c => c.charCodeAt(0)).buffer);
    const rows = Math.ceil(words.length / 4096);
    const padded = new Uint32Array(rows * 4096); padded.set(words);
    const tex = device.createTexture({ size: [4096, rows], format: 'r32uint', usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST });
    device.queue.writeTexture({ texture: tex }, padded, { bytesPerRow: 4096 * 4, rowsPerImage: rows }, [4096, rows]);
    const io = new Float32Array(pts.length * 8);
    const iv = new Int32Array(io.buffer);
    pts.forEach(([x, y, t], k) => { iv[8 * k] = x; iv[8 * k + 1] = y; io[8 * k + 2] = t; });
    const buf = device.createBuffer({ size: io.byteLength, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC });
    device.queue.writeBuffer(buf, 0, io);
    const read = device.createBuffer({ size: io.byteLength, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
    const pipeline = await device.createComputePipelineAsync({ layout: 'auto', compute: { module, entryPoint: 'probe', constants: { MODEL: 1, MODEL_PROBE: mprobe } } });
    const group = device.createBindGroup({ layout: pipeline.getBindGroupLayout(0), entries: [{ binding: 0, resource: { buffer: buf } }, { binding: 1, resource: tex.createView() }] });
    const enc = device.createCommandEncoder();
    const pass = enc.beginComputePass(); pass.setPipeline(pipeline); pass.setBindGroup(0, group); pass.dispatchWorkgroups(Math.ceil(pts.length / 64)); pass.end();
    enc.copyBufferToBuffer(buf, 0, read, 0, io.byteLength);
    device.queue.submit([enc.finish()]);
    await read.mapAsync(GPUMapMode.READ);
    const o = new Float32Array(read.getMappedRange().slice(0));
    const ou = new Uint32Array(o.buffer);
    const err = await device.popErrorScope();
    return { err: err?.message, out: pts.map((_, k) => [ou[8 * k + 4], o[8 * k + 5], o[8 * k + 6], ou[8 * k + 7]]) };
}, { code, pts, model: model.toString('base64'), mprobe: Number(process.env.OBC_MPROBE ?? 0) });
await browser.close(); close();
if (result.errors) { console.log(result.errors.slice(0, 10).join('\n')); process.exit(1); }
const rows = pts.map(([x, y, t], k) => {
    const [label, h, kind, block] = result.out[k];
    return { cell: [x >> 14, y >> 14], q: [(x & 16383) * 16, (y & 16383) * 16], label: label === 0xffffffff ? null : label, t, h: block ? h : null, kind: block ? kind : null };
});
writeFileSync(out, JSON.stringify(rows));
console.log(`${rows.length} points, ${rows.filter(r => r.h !== null).length} in a block, err ${result.err}`);
