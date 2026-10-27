import type { Coordinate } from '../../planner/geo';
import { worldPixel } from '../../planner/layers/mercator';
import { UNITS } from '../frame/space';
import type { Gpu } from '../gpu/device';
import { layouts } from '../gpu/layouts';
import { wgsl } from '../gpu/wgsl';
import { DEPTH, sceneFormat } from '../resolve/targets';
import { metresPerUnit } from '../terrain/bins';

/** Words of a point, as `Point` of route.wgsl: x, y (int32 world units) and its colour as sRGB bytes; the stride of its vec2i adds one. */
export const POINT_WORDS = 4;
/** The line bends at most this far from the surface between two points. */
const STEP_M = 5;
/** The size of a pin image in image pixels, as PIN of route.wgsl: twice its output pixels. */
export const PIN_SIZE: [number, number] = [72, 96];

/** A pin on the ground: the bottom centre of its image stands on the coordinate. */
export interface RoutePin { coordinate: Coordinate; image: GPUCopyExternalImageSource }

/**
 * The points of a route in riding order: its vertices, and between them points at most STEP_M apart, so
 * that the ribbon follows the surface. A point takes the colour of the edge that ends at it, the first one that of the first edge.
 */
export function routePoints(route: readonly Coordinate[], colours: readonly string[]): Uint32Array<ArrayBuffer> {
    const words: number[] = [];
    const push = (x: number, y: number, colour: string) => words.push(Math.round(x), Math.round(y), packed(colour), 0);
    route.forEach((coordinate, i) => {
        const [x, y] = worldPixel(coordinate, UNITS);
        if (!i) return push(x, y, colours[0]);
        const [lx, ly] = worldPixel(route[i - 1], UNITS), metres = Math.hypot(x - lx, y - ly) * metresPerUnit((y + ly) / 2);
        const steps = Math.ceil(metres / STEP_M);
        for (let k = 1; k <= steps; k++) push(lx + (x - lx) * k / steps, ly + (y - ly) * k / steps, colours[i - 1]);
    });
    return new Uint32Array(new Int32Array(words).buffer);
}

/** '#rrggbb' as the bytes of WGSL's unpack4x8unorm: red lowest. */
function packed(hex: string): number {
    return parseInt(hex.slice(5, 7) + hex.slice(3, 5) + hex.slice(1, 3), 16);
}

interface Resources {
    gpu: Gpu;
    line: GPURenderPipeline;
    pin: GPURenderPipeline;
    layout: GPUBindGroupLayout;
    frame: GPUBuffer;
    empty: GPUBindGroup;
    sampler: GPUSampler;
    points?: GPUBuffer;
    pins?: GPUBuffer;
    images?: GPUTexture;
    group?: GPUBindGroup;
}

/** The planned route on the drawn surface and the pins along it, with route.wgsl. */
export class Route {
    /** Whether the route changed since the last draw. */
    newContent = false;
    private points = new Uint32Array(0);
    private pins: readonly RoutePin[] = [];
    private resources?: Resources;
    /** Whether the route is not on the GPU yet. */
    private fresh = true;

    /** The route in riding order with the colour of each edge; fewer than two points draw none. */
    set(route: readonly Coordinate[], colours: readonly string[], pins: readonly RoutePin[]): void {
        this.points = route.length < 2 ? new Uint32Array(0) : routePoints(route, colours);
        this.pins = pins;
        this.fresh = this.newContent = true;
    }

    /** `frame` is the frame uniform of the eye's view. */
    async attach(gpu: Gpu, frame: GPUBuffer): Promise<void> {
        this.resources = undefined;
        const { line, pin, layout, empty } = await gpu.once(routePipelines);
        this.resources = {
            gpu, line, pin, layout, frame, empty: gpu.device.createBindGroup({ layout: empty, entries: [] }),
            sampler: gpu.device.createSampler({ magFilter: 'linear', minFilter: 'linear' }),
        };
        this.fresh = true;
    }

    draw(pass: GPURenderPassEncoder, surface: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        this.newContent = false;
        if (!r) return;
        if (this.fresh) this.write(r);
        if (!r.group) return;
        [r.group, surface, r.empty, features].forEach((group, k) => pass.setBindGroup(k, group));
        if (this.points.length) {
            pass.setPipeline(r.line);
            pass.draw(2 * this.points.length / POINT_WORDS);
        }
        if (this.pins.length) {
            pass.setPipeline(r.pin);
            pass.draw(6, this.pins.length);
        }
    }

    private write(r: Resources): void {
        this.fresh = false;
        const { gpu } = r, { device } = gpu;
        r.group = undefined;
        if (!this.points.length && !this.pins.length) return;
        // The bindings take at least one point, pin and image.
        const points = this.points.length ? this.points : new Uint32Array(POINT_WORDS), pins = Math.max(this.pins.length, 1);
        if (!r.points || r.points.size < points.byteLength) {
            if (r.points) gpu.destroy('route', r.points);
            r.points = gpu.buffer('route', points.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        }
        device.queue.writeBuffer(r.points, 0, points);
        if (!r.pins || r.pins.size < pins * 8) {
            if (r.pins) gpu.destroy('route', r.pins);
            r.pins = gpu.buffer('route', pins * 8, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
        }
        device.queue.writeBuffer(r.pins, 0, new Int32Array(this.pins.flatMap(({ coordinate }) => worldPixel(coordinate, UNITS).map(Math.round))));
        if (r.images?.depthOrArrayLayers !== pins) {
            if (r.images) gpu.destroy('route', r.images);
            r.images = gpu.texture('route', {
                size: [...PIN_SIZE, pins], format: 'rgba8unorm',
                usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
            }, PIN_SIZE[0] * PIN_SIZE[1] * 4 * pins);
        }
        const images = r.images;
        this.pins.forEach(({ image }, k) => device.queue.copyExternalImageToTexture(
            { source: image }, { texture: images, origin: [0, 0, k], premultipliedAlpha: true }, PIN_SIZE));
        // The shader counts the points and the pins by the bound size, so a shorter route binds less of each buffer.
        r.group = device.createBindGroup({ layout: r.layout, entries: [
            { binding: 0, resource: { buffer: r.frame } }, { binding: 1, resource: { buffer: r.points, size: points.byteLength } },
            { binding: 2, resource: { buffer: r.pins, size: pins * 8 } }, { binding: 3, resource: images.createView({ dimension: '2d-array' }) },
            { binding: 4, resource: r.sampler },
        ] });
    }
}

async function routePipelines(gpu: Gpu) {
    const { device } = gpu, { surface, features } = gpu.once(layouts), { VERTEX, FRAGMENT } = GPUShaderStage;
    const layout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: VERTEX | FRAGMENT, buffer: { type: 'uniform' } },
        { binding: 1, visibility: VERTEX, buffer: { type: 'read-only-storage' } },
        { binding: 2, visibility: VERTEX, buffer: { type: 'read-only-storage' } },
        { binding: 3, visibility: FRAGMENT, texture: { viewDimension: '2d-array' } },
        { binding: 4, visibility: FRAGMENT, sampler: {} },
    ] });
    const empty = device.createBindGroupLayout({ entries: [] }), module = device.createShaderModule({ label: 'route', code: wgsl('route/route.wgsl') });
    const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout, surface, empty, features] });
    // Premultiplied: what is drawn covers by its alpha and adds the rest of its light.
    const blend: GPUBlendComponent = { srcFactor: 'one', dstFactor: 'one-minus-src-alpha' };
    const pipeline = (name: 'route' | 'pin', topology: GPUPrimitiveTopology) => device.createRenderPipelineAsync({
        label: name, layout: pipelineLayout,
        vertex: { module, entryPoint: `${name}_vertex` },
        fragment: { module, entryPoint: `${name}_fragment`, targets: [{ format: sceneFormat(gpu), blend: { color: blend, alpha: blend } }] },
        primitive: { topology },
        depthStencil: { format: DEPTH, depthWriteEnabled: false, depthCompare: 'greater' },
    });
    const [line, pin] = await Promise.all([pipeline('route', 'triangle-strip'), pipeline('pin', 'triangle-list')]);
    return { line, pin, layout, empty };
}
