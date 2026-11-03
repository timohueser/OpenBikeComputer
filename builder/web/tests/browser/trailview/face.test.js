import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

// face.wgsl as the ground look reads it, with value noise of a hash in place of look.wgsl's table: per sample a
// normal, a point and a unit step in the face, it gives face_noise and the plan's noise at the point and one step on.
const FACE = readFileSync(new URL('../../../src/lib/trailview/ground/face.wgsl', import.meta.url), 'utf8').replace(/^\/\/ #include .*$/gm, '');
const STEP_M = 0.02;
const CODE = `${FACE}
fn hash(q: vec2f) -> f32 {
    var p3 = fract(q.xyx * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}
fn noise(q: vec2f) -> f32 {
    let i = floor(q);
    let u = (q - i) * (q - i) * (3.0 - 2.0 * (q - i));
    return mix(mix(hash(i), hash(i + vec2f(1.0, 0.0)), u.x), mix(hash(i + vec2f(0.0, 1.0)), hash(i + vec2f(1.0)), u.x), u.y);
}
@group(0) @binding(0) var<storage, read> samples: array<f32>;
@group(0) @binding(1) var<storage, read_write> out: array<vec4f>;
fn at(k: u32, j: u32) -> vec3f {
    let b = 9u * k + 3u * j;
    return vec3f(samples[b], samples[b + 1u], samples[b + 2u]);
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= arrayLength(&out)) {
        return;
    }
    let n = at(id.x, 0u);
    let p = at(id.x, 1u);
    let q = p + ${STEP_M} * at(id.x, 2u);
    out[id.x] = vec4f(face_noise(facing_of(n, p), 1.0, 0.0), face_noise(facing_of(n, q), 1.0, 0.0), noise(p.xy), noise(q.xy));
}`;

const DEG = Math.PI / 180;
const SLOPES = [0, 30, 45, 60, 75], BEARINGS = [0, 20, 45, 70], DIRECTIONS = ['fall', 'contour'], POINTS = 2048;

/** The samples of one face: its normal, points over 40 m of it, and the unit step along its fall line or contour. */
function face(slope, bearing, direction) {
    const [s, c] = [Math.sin(slope * DEG), Math.cos(slope * DEG)], [sb, cb] = [Math.sin(bearing * DEG), Math.cos(bearing * DEG)];
    const n = [s * cb, s * sb, c], fall = [-c * cb, -c * sb, s], contour = [-sb, cb, 0];
    const step = direction === 'fall' ? fall : contour;
    let seed = 7 + slope * 101 + bearing * 13 + direction.length;
    const random = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    return Array.from({ length: POINTS }, () => {
        const [a, b] = [40 * random(), 40 * random()];
        return [...n, ...[0, 1, 2].map(i => [130, 270, 1500][i] + a * contour[i] + b * fall[i]), ...step];
    }).flat();
}

const rms = values => Math.sqrt(values.reduce((sum, v) => sum + v * v, 0) / values.length);
const deviation = values => rms(values.map(v => v - values.reduce((sum, w) => sum + w, 0) / values.length));

test('a steep face takes the ground detail in its own plane, and flat ground keeps the plan', async ({ page }) => {
    const cases = SLOPES.flatMap(slope => BEARINGS.flatMap(bearing => DIRECTIONS.map(direction => ({ slope, bearing, direction }))));
    const input = cases.flatMap(({ slope, bearing, direction }) => face(slope, bearing, direction));
    await page.goto('/third-party-licenses.txt');
    const values = await page.evaluate(async ({ code, input }) => {
        const device = await (await navigator.gpu.requestAdapter()).requestDevice();
        const pipeline = device.createComputePipeline({ layout: 'auto', compute: { module: device.createShaderModule({ code }), entryPoint: 'main' } });
        const samples = device.createBuffer({ size: input.length * 4, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST });
        device.queue.writeBuffer(samples, 0, new Float32Array(input));
        const size = (input.length / 9) * 16;
        const out = device.createBuffer({ size, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC });
        const read = device.createBuffer({ size, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
        const encoder = device.createCommandEncoder();
        const pass = encoder.beginComputePass();
        pass.setPipeline(pipeline);
        pass.setBindGroup(0, device.createBindGroup({ layout: pipeline.getBindGroupLayout(0), entries: [{ binding: 0, resource: { buffer: samples } }, { binding: 1, resource: { buffer: out } }] }));
        pass.dispatchWorkgroups(Math.ceil(input.length / 9 / 64));
        pass.end();
        encoder.copyBufferToBuffer(out, 0, read, 0, size);
        device.queue.submit([encoder.finish()]);
        await read.mapAsync(GPUMapMode.READ);
        return Array.from(new Float32Array(read.getMappedRange()));
    }, { code: CODE, input });
    const results = cases.map((c, k) => {
        const v = values.slice(k * POINTS * 4, (k + 1) * POINTS * 4);
        const column = j => Array.from({ length: POINTS }, (_, i) => v[4 * i + j]);
        const rate = (a, b) => rms(column(b).map((x, i) => (x - column(a)[i]) / STEP_M));
        return { ...c, face: column(0), plan: column(2), rate: rate(0, 1), planRate: rate(2, 3) };
    });
    // The noise's rate of change per metre of flat ground: its density.
    const flat = results.filter(r => r.slope === 0), density = rms(flat.map(r => r.rate));
    for (const r of results) {
        const name = `${r.slope}° facing ${r.bearing}°, along the ${r.direction}`;
        // Up to 37° the plan draws alone: flat ground looks as before and pays for no other projection.
        if (r.slope <= 30) expect(r.face, name).toEqual(r.plan);
        // A steep face's detail stretches at most √2 in any direction (along the contour of a face 45° off both axes),
        // where the plan's stretches a 60° face 2 times along its fall line and a 75° face 3.9 times.
        if (r.slope >= 60) {
            expect(r.rate, name).toBeGreaterThan(density / 1.6);
            if (r.direction === 'fall') expect(r.planRate, name).toBeLessThan(density / 1.6);
        }
        // Where the projections blend, the detail keeps the contrast of one noise.
        expect(deviation(r.face) / deviation(r.plan), name).toBeGreaterThan(0.85);
        expect(deviation(r.face) / deviation(r.plan), name).toBeLessThan(1.15);
    }
});
