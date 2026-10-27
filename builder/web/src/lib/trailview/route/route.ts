import type { Coordinate } from '../../planner/geo';
import { gradeBands, gradeThresholds } from '../../planner/grade-data';
import { worldPixel } from '../../planner/layers/mercator';
import { UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { metresPerUnit } from '../terrain/bins';

/** Words of a point, as `Point` of route.wgsl: x, y (int32 world units), metres along, the first and last point of its grade window; the stride of its vec2i adds one. */
export const POINT_WORDS = 6;
/** The line bends at most this far from the surface between two points. */
const STEP_M = 5;
/** The grade is the rise over this many metres to each side of a point, as profileGrades of grade-data.ts. */
const WINDOW_M = 50;
/** As the planner's map: the first day's line. */
const LINE = '#cc2a93';
const STYLE_BYTES = 15 * 16;

/**
 * The points of a route in riding order: its vertices, and between them points at most STEP_M apart, so
 * that the ribbon follows the surface; each with its metres along and its grade window.
 */
export function routePoints(route: readonly Coordinate[]): Uint32Array<ArrayBuffer> {
    const xy: number[] = [], along: number[] = [];
    for (const coordinate of route) {
        const [x, y] = worldPixel(coordinate, UNITS), n = along.length;
        if (!n) {
            xy.push(x, y);
            along.push(0);
            continue;
        }
        const [lx, ly] = [xy[2 * n - 2], xy[2 * n - 1]], metres = Math.hypot(x - lx, y - ly) * metresPerUnit((y + ly) / 2);
        const steps = Math.ceil(metres / STEP_M);
        for (let k = 1; k <= steps; k++) {
            xy.push(lx + (x - lx) * k / steps, ly + (y - ly) * k / steps);
            along.push(along[n - 1] + metres * k / steps);
        }
    }
    const words = new Uint32Array(along.length * POINT_WORDS), ints = new Int32Array(words.buffer), floats = new Float32Array(words.buffer);
    let behind = 0, ahead = 0;
    along.forEach((metres, i) => {
        while (metres - along[behind] > WINDOW_M) behind++;
        while (ahead + 1 < along.length && along[ahead + 1] - metres <= WINDOW_M) ahead++;
        ints.set([Math.round(xy[2 * i]), Math.round(xy[2 * i + 1])], i * POINT_WORDS);
        floats[i * POINT_WORDS + 2] = metres;
        words.set([behind, ahead], i * POINT_WORDS + 3);
    });
    return words;
}

/** `Style` of route.wgsl. The shaders take an sRGB colour squared as linear light. */
function styleOf(graded: boolean): Float32Array<ArrayBuffer> {
    const linear = (hex: string) => [1, 3, 5].map(i => (parseInt(hex.slice(i, i + 2), 16) / 255) ** 2);
    const style = new Float32Array(STYLE_BYTES / 4);
    style.set([...linear(LINE), graded ? 1 : 0, ...gradeThresholds], 0);
    style.set(gradeBands.flatMap(({ color }) => [...linear(color), 1]), 12);
    return style;
}

interface Resources {
    gpu: Gpu;
    pipeline: GPURenderPipeline;
    layout: GPUBindGroupLayout;
    frame: GPUBuffer;
    style: GPUBuffer;
    empty: GPUBindGroup;
    points?: GPUBuffer;
    group?: GPUBindGroup;
}

/** The planned route on the drawn surface, with route.wgsl. */
export class Route {
    /** Whether the route or its look changed since the last draw. */
    newContent = false;
    private points = new Uint32Array(0);
    private graded = false;
    private resources?: Resources;
    /** Whether the points and the style are not on the GPU yet. */
    private fresh = true;

    /** The route in riding order; fewer than two points draw none. `graded` colours it by its grade. */
    set(route: readonly Coordinate[], graded: boolean): void {
        this.points = route.length < 2 ? new Uint32Array(0) : routePoints(route);
        this.graded = graded;
        this.fresh = this.newContent = true;
    }

    /** `frame` is the frame uniform of the eye's view. */
    async attach(gpu: Gpu, frame: GPUBuffer): Promise<void> {
        this.resources = undefined;
        const { pipeline, layout, empty } = await gpu.once(routePipeline);
        this.resources = {
            gpu, pipeline, layout, frame, style: gpu.buffer('route', STYLE_BYTES, GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST),
            empty: gpu.device.createBindGroup({ layout: empty, entries: [] }),
        };
        this.fresh = true;
    }

    draw(pass: GPURenderPassEncoder, surface: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        if (!r) return;
        if (this.fresh) this.write(r);
        this.newContent = false;
        if (!r.group || !this.points.length) return;
        pass.setPipeline(r.pipeline);
        [r.group, surface, r.empty, features].forEach((group, k) => pass.setBindGroup(k, group));
        pass.draw(2 * this.points.length / POINT_WORDS);
    }

    private write(r: Resources): void {
        this.fresh = false;
        const { gpu } = r;
        gpu.device.queue.writeBuffer(r.style, 0, styleOf(this.graded));
        if (!this.points.length) return;
        if (!r.points || r.points.size < this.points.byteLength) {
            if (r.points) gpu.destroy('route', r.points);
            r.points = gpu.buffer('route', this.points.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        }
        gpu.device.queue.writeBuffer(r.points, 0, this.points);
        // The shader counts the points by the bound size, so a shorter route binds less of the buffer.
        r.group = gpu.device.createBindGroup({ layout: r.layout, entries: [
            { binding: 0, resource: { buffer: r.frame } }, { binding: 1, resource: { buffer: r.points, size: this.points.byteLength } },
            { binding: 2, resource: { buffer: r.style } },
        ] });
    }
}

async function routePipeline(gpu: Gpu) {
    const { device } = gpu, { surface, features } = gpu.once(layouts), VF = GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT;
    const layout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: VF, buffer: { type: 'uniform' } }, { binding: 1, visibility: GPUShaderStage.VERTEX, buffer: { type: 'read-only-storage' } },
        { binding: 2, visibility: VF, buffer: { type: 'uniform' } },
    ] });
    const empty = device.createBindGroupLayout({ entries: [] }), module = device.createShaderModule({ label: 'route', code: wgsl('route/route.wgsl') });
    const blend: GPUBlendComponent = { srcFactor: 'one', dstFactor: 'one-minus-src-alpha' };
    const pipeline = await device.createRenderPipelineAsync({
        label: 'route', layout: device.createPipelineLayout({ bindGroupLayouts: [layout, surface, empty, features] }),
        vertex: { module, entryPoint: 'route_vertex' },
        fragment: { module, entryPoint: 'route_fragment', targets: [{ format: sceneFormat(gpu), blend: { color: blend, alpha: blend } }] },
        primitive: { topology: 'triangle-strip' },
        depthStencil: { format: DEPTH, depthWriteEnabled: false, depthCompare: 'greater' },
    });
    return { pipeline, layout, empty };
}
