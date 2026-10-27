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
/** The side of a badge image in image pixels, as BADGE_PIXELS of route.wgsl: twice its output pixels. */
export const BADGE_SIZE = 64;
/** Words of a pin, as `Pin` of route.wgsl. */
const PIN_WORDS = 4;
/** The share of its light that a pin shows where something hides it. */
const HIDDEN_PIN = 0.35;

/** A pin on the ground: a stem over a ring in its colour, with its badge on top, or without one a sphere in its colour. */
export interface RoutePin { coordinate: Coordinate; colour: string; badge?: GPUCopyExternalImageSource }

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
    /** The pins where they are in sight, and where something hides them. */
    pin: GPURenderPipeline[];
    layout: GPUBindGroupLayout;
    frame: GPUBuffer;
    empty: GPUBindGroup;
    sampler: GPUSampler;
    points?: GPUBuffer;
    pins?: GPUBuffer;
    badges?: GPUTexture;
    group?: GPUBindGroup;
}

/** The planned route on the drawn surface and the pins on the ground, with route.wgsl. */
export class Route {
    /** Whether the route or a pin changed since the last draw. */
    newContent = false;
    private points = new Uint32Array(0);
    private pins: readonly RoutePin[] = [];
    /** The pin that follows the rider's pointer, after the others. */
    private marker?: RoutePin;
    private resources?: Resources;
    /** What is not on the GPU yet. */
    private fresh = { points: true, pins: true, badges: true };

    /** The route in riding order with the colour of each edge; fewer than two points draw none. */
    set(route: readonly Coordinate[], colours: readonly string[]): void {
        this.points = route.length < 2 ? new Uint32Array(0) : routePoints(route, colours);
        this.fresh.points = this.newContent = true;
    }

    setPins(pins: readonly RoutePin[]): void {
        this.pins = pins;
        this.fresh.pins = this.fresh.badges = this.newContent = true;
    }

    /** A pin without a badge, which moves without loading the badges again. */
    setMarker(marker: Omit<RoutePin, 'badge'> | undefined): void {
        this.marker = marker;
        this.fresh.pins = this.newContent = true;
    }

    /** `frame` is the frame uniform of the eye's view. */
    async attach(gpu: Gpu, frame: GPUBuffer): Promise<void> {
        this.resources = undefined;
        const { line, pin, layout, empty } = await gpu.once(routePipelines);
        this.resources = {
            gpu, line, pin, layout, frame, empty: gpu.device.createBindGroup({ layout: empty, entries: [] }),
            sampler: gpu.device.createSampler({ magFilter: 'linear', minFilter: 'linear' }),
        };
        this.fresh = { points: true, pins: true, badges: true };
    }

    draw(pass: GPURenderPassEncoder, surface: GPUBindGroup, features: GPUBindGroup): void {
        const r = this.resources;
        this.newContent = false;
        if (!r) return;
        if (this.fresh.points || this.fresh.pins || this.fresh.badges) this.write(r);
        if (!r.group) return;
        [r.group, surface, r.empty, features].forEach((group, k) => pass.setBindGroup(k, group));
        if (this.points.length) {
            pass.setPipeline(r.line);
            pass.draw(2 * this.points.length / POINT_WORDS);
        }
        const pins = this.pins.length + (this.marker ? 1 : 0);
        if (pins) for (const pipeline of r.pin) {
            pass.setPipeline(pipeline);
            // A ring, a stem and a head of two triangles each.
            pass.draw(18, pins);
        }
    }

    private write(r: Resources): void {
        const { gpu } = r, { device } = gpu;
        const pins = this.marker ? [...this.pins, this.marker] : this.pins;
        // The bindings take at least one point, pin and badge.
        const points = this.points.length ? this.points : new Uint32Array(POINT_WORDS);
        const words = new Int32Array(Math.max(pins.length, 1) * PIN_WORDS), layers = Math.max(this.pins.length, 1);
        if (this.fresh.points) {
            if (!r.points || r.points.size < points.byteLength) {
                if (r.points) gpu.destroy('route', r.points);
                r.points = gpu.buffer('route', points.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
            }
            device.queue.writeBuffer(r.points, 0, points);
        }
        if (this.fresh.pins) {
            // x, y, the colour, and the layer of the badge plus one, or 0 for a sphere.
            pins.forEach(({ coordinate, colour, badge }, k) => words.set([...worldPixel(coordinate, UNITS).map(Math.round), packed(colour), badge ? k + 1 : 0], k * PIN_WORDS));
            if (!r.pins || r.pins.size < words.byteLength) {
                if (r.pins) gpu.destroy('route', r.pins);
                r.pins = gpu.buffer('route', words.byteLength, GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_DST);
            }
            device.queue.writeBuffer(r.pins, 0, words);
        }
        if (this.fresh.badges) {
            if (r.badges?.depthOrArrayLayers !== layers) {
                if (r.badges) gpu.destroy('route', r.badges);
                r.badges = gpu.texture('route', {
                    size: [BADGE_SIZE, BADGE_SIZE, layers], format: 'rgba8unorm',
                    usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
                }, BADGE_SIZE * BADGE_SIZE * 4 * layers);
            }
            const badges = r.badges;
            this.pins.forEach(({ badge }, k) => badge && device.queue.copyExternalImageToTexture(
                { source: badge }, { texture: badges, origin: [0, 0, k], premultipliedAlpha: true }, [BADGE_SIZE, BADGE_SIZE]));
        }
        this.fresh = { points: false, pins: false, badges: false };
        // The shader counts the points by the bound size, so a shorter route binds less of the buffer.
        r.group = device.createBindGroup({ layout: r.layout, entries: [
            { binding: 0, resource: { buffer: r.frame } }, { binding: 1, resource: { buffer: r.points!, size: points.byteLength } },
            { binding: 2, resource: { buffer: r.pins! } }, { binding: 3, resource: r.badges!.createView({ dimension: '2d-array' }) },
            { binding: 4, resource: r.sampler },
        ] });
    }
}

async function routePipelines(gpu: Gpu) {
    const { device } = gpu, { surface, features } = gpu.once(layouts), { VERTEX, FRAGMENT } = GPUShaderStage;
    const layout = device.createBindGroupLayout({ entries: [
        { binding: 0, visibility: VERTEX | FRAGMENT, buffer: { type: 'uniform' } },
        { binding: 1, visibility: VERTEX, buffer: { type: 'read-only-storage' } },
        { binding: 2, visibility: VERTEX | FRAGMENT, buffer: { type: 'read-only-storage' } },
        { binding: 3, visibility: FRAGMENT, texture: { viewDimension: '2d-array' } },
        { binding: 4, visibility: FRAGMENT, sampler: {} },
    ] });
    const empty = device.createBindGroupLayout({ entries: [] }), module = device.createShaderModule({ label: 'route', code: wgsl('route/route.wgsl') });
    const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout, surface, empty, features] });
    // Premultiplied: what is drawn covers by its alpha and adds the rest of its light.
    const blend: GPUBlendComponent = { srcFactor: 'one', dstFactor: 'one-minus-src-alpha' };
    const pipeline = (name: 'route' | 'pin', topology: GPUPrimitiveTopology, hidden = false) => device.createRenderPipelineAsync({
        label: name, layout: pipelineLayout,
        vertex: { module, entryPoint: `${name}_vertex` },
        fragment: {
            module, entryPoint: `${name}_fragment`, targets: [{ format: sceneFormat(gpu), blend: { color: blend, alpha: blend } }],
            constants: hidden ? { SHOWN: HIDDEN_PIN } : {},
        },
        primitive: { topology },
        depthStencil: { format: DEPTH, depthWriteEnabled: false, depthCompare: hidden ? 'less-equal' : 'greater' },
    });
    const [line, ...pin] = await Promise.all([pipeline('route', 'triangle-strip'), pipeline('pin', 'triangle-list'), pipeline('pin', 'triangle-list', true)]);
    return { line, pin, layout, empty };
}
